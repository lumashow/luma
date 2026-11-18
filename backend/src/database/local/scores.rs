use sqlx::SqlitePool;

use crate::database::local::venue_access::{AuthorizedVenue, VenueAccess, Write};
use crate::models::scores::{Score, ScoreSummary};

pub mod rows;

/// The one order a score listing takes: newest-created first.
///
/// `datetime()` rather than the raw column because the two ways a score row is
/// born spell the same instant differently — a local insert takes SQLite's
/// `CURRENT_TIMESTAMP` (`2026-08-29 12:00:00`) while a row written by the
/// current schema carries RFC 3339 (`2026-08-29T12:00:00Z`) — and `'T' > ' '`
/// in text order, so raw comparison files every new-format row above every
/// older one whatever the clock says. Parsing both to one canonical form is
/// what makes the comparison mean what it reads as.
///
/// The tie-break is `rowid`, because `CURRENT_TIMESTAMP` only resolves to the
/// second and two scores minted in one are otherwise unordered. `rowid` is
/// this database's insert order — the one thing that still says which of the
/// two it learned about last — where the uuid `id` would sort at random. It
/// is the same key [`score_ordinal`] ranks by, so the list is exactly the
/// ordinal ladder read upside down: `#3` can never appear below `#2`.
///
/// Ordering lives here and nowhere else: a client that re-sorts a listing is a
/// second definition of "newest", and the two drift the moment one of them is
/// tuned.
macro_rules! newest_first {
    () => {
        "ORDER BY datetime(score.created_at) DESC, score.rowid DESC"
    };
}

/// The seam's display handle within its venue — `#1` is the oldest score.
///
/// Shares [`newest_first`]'s parse of `created_at` for the same reason, and
/// its `rowid` tie-break so the two rankings are reverses of one another
/// rather than two nearly-equal opinions.
macro_rules! score_ordinal {
    () => {
        "ROW_NUMBER() OVER (
                    PARTITION BY score.venue_id
                    ORDER BY datetime(score.created_at), score.rowid
                ) AS ordinal"
    };
}

/// The provenance columns every score listing carries: when the score was last
/// written, and what its agent threads have cost.
///
/// The time is the score's `authored_at` rather than its `updated_at`, because
/// the row moves for reasons that are not authorship. `save_score` sets it
/// exactly when something changed.
///
/// Correlated subqueries rather than joins, because a one-to-many join would
/// multiply the counts the listing already groups for.
///
/// Assumes the listing's score is aliased `score`. A macro rather than a
/// `const` so each listing stays one `concat!`-ed literal: the statements
/// carry no runtime input, and keeping them `&'static str` is what lets sqlx
/// take them without an assertion that they are safe to run.
macro_rules! provenance {
    () => {
        "score.authored_at AS last_authored_at,
                (SELECT SUM(usage.cost_usd)
                   FROM agent_thread_usage usage
                   JOIN agent_threads thread ON thread.id = usage.thread_id
                  WHERE thread.score_id = score.id) AS cost_usd,
                (SELECT COALESCE(SUM(usage.input_tokens + usage.output_tokens
                                     + usage.cache_creation_tokens
                                     + usage.cache_read_tokens), 0)
                   FROM agent_thread_usage usage
                   JOIN agent_threads thread ON thread.id = usage.thread_id
                  WHERE thread.score_id = score.id) AS total_tokens"
    };
}

/// List scores for a track inside the guard's one admitted venue.
pub async fn list_scores_for_track(
    access: &mut impl AuthorizedVenue,
    track_id: &str,
) -> Result<Vec<ScoreSummary>, String> {
    // The venue join is LEFT so it decorates without filtering: this query's
    // result set is settled by the guard above it, not by the name lookup.
    const ONE_VENUE: &str = concat!(
        "SELECT score.id, score.uid, score.venue_id, venue.name AS venue_name, score.name,
                ",
        score_ordinal!(),
        ",
                score.uid IS ? AS editable,
                (SELECT COUNT(*) FROM clips WHERE clips.score_id = score.id) AS annotation_count,
                ",
        provenance!(),
        ",
                score.created_at, score.updated_at
         FROM scores score
         LEFT JOIN venues venue ON venue.id = score.venue_id
         WHERE score.track_id = ? AND score.venue_id = ?
         ",
        newest_first!()
    );
    sqlx::query_as::<_, ScoreSummary>(ONE_VENUE)
        .bind(access.principal().to_owned())
        .bind(track_id)
        .bind(access.venue_id().to_owned())
        .fetch_all(&mut *access.connection())
        .await
        .map_err(|e| format!("Failed to list scores for track: {}", e))
}

/// Cross-venue score picker view filtered in one statement by the current app
/// admission: the venues `auth_venue_access` lets the principal open. It never returns sealed rows from another principal.
pub async fn list_accessible_scores_for_track(
    pool: &SqlitePool,
    track_id: &str,
) -> Result<Vec<ScoreSummary>, String> {
    const ACCESSIBLE: &str = concat!(
        "SELECT score.id, score.uid, score.venue_id, venue.name AS venue_name, score.name,
                ",
        score_ordinal!(),
        ",
                score.uid IS access.principal AS editable,
                (SELECT COUNT(*) FROM clips WHERE clips.score_id = score.id) AS annotation_count,
                ",
        provenance!(),
        ",
                score.created_at, score.updated_at
         FROM scores score
         JOIN venues venue ON venue.id = score.venue_id
         JOIN auth_venue_access access ON access.venue_id = score.venue_id
         WHERE score.track_id = ?
         ",
        newest_first!()
    );
    sqlx::query_as::<_, ScoreSummary>(ACCESSIBLE)
        .bind(track_id)
        .fetch_all(pool)
        .await
        .map_err(|error| format!("Failed to list accessible scores for track: {error}"))
}

/// What [`find_own_scores`] narrows by. Every field narrows further; none set
/// lists every score of the principal's.
#[derive(Debug, Default, Clone)]
pub struct ScoreSearch {
    /// Words that must each appear in the track's title or artist, the
    /// venue's name or the score's name.
    pub text: Option<String>,
    /// A track id: scores of that audio, whichever track row holds it.
    pub track: Option<String>,
    /// A venue id, or part of its name.
    pub venue: Option<String>,
    /// Part of the name of a clip the score holds.
    pub uses: Option<String>,
    /// A score left out: the one the caller already has open.
    pub except: Option<String>,
    /// A track id whose scores are listed before the rest.
    pub first: Option<String>,
}

/// One score of the library, as a search lists it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, serde::Serialize)]
pub struct FoundScore {
    pub id: String,
    pub name: Option<String>,
    pub track_id: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub venue_id: String,
    pub venue_name: String,
    pub clips: i64,
    pub edited_at: Option<String>,
}

/// Search the scores the current principal wrote, in every venue the
/// admission still lets them open, last written first. A score someone else
/// wrote in one of those venues is not listed.
pub async fn find_own_scores(
    pool: &SqlitePool,
    search: &ScoreSearch,
    limit: u32,
) -> Result<Vec<FoundScore>, String> {
    /// `%word%` with LIKE's own wildcards escaped, so a name with `_` or `%`
    /// in it matches itself.
    fn contains(word: &str) -> String {
        let mut pattern = String::from("%");
        for character in word.chars() {
            if matches!(character, '%' | '_' | '\\') {
                pattern.push('\\');
            }
            pattern.push(character);
        }
        pattern.push('%');
        pattern
    }
    const SAME_AUDIO: &str = "track.track_hash = (SELECT track_hash FROM tracks WHERE id = ";

    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT score.id, score.name, score.track_id, track.title, track.artist,
                score.venue_id, venue.name AS venue_name,
                (SELECT COUNT(*) FROM clips WHERE clips.score_id = score.id) AS clips,
                COALESCE(score.authored_at, score.updated_at) AS edited_at
         FROM scores score
         JOIN venues venue ON venue.id = score.venue_id
         JOIN auth_venue_access access ON access.venue_id = score.venue_id
         JOIN tracks track ON track.id = score.track_id
         WHERE score.uid IS access.principal",
    );
    if let Some(except) = &search.except {
        query.push(" AND score.id != ").push_bind(except);
    }
    if let Some(track) = &search.track {
        query
            .push(" AND ")
            .push(SAME_AUDIO)
            .push_bind(track)
            .push(")");
    }
    if let Some(venue) = &search.venue {
        query
            .push(" AND (venue.id = ")
            .push_bind(venue)
            .push(" OR venue.name LIKE ")
            .push_bind(contains(venue))
            .push(" ESCAPE '\\')");
    }
    for word in search.text.iter().flat_map(|text| text.split_whitespace()) {
        query.push(" AND (");
        for (index, column) in ["track.title", "track.artist", "venue.name", "score.name"]
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                query.push(" OR ");
            }
            query
                .push(column)
                .push(" LIKE ")
                .push_bind(contains(word))
                .push(" ESCAPE '\\'");
        }
        query.push(")");
    }
    if let Some(uses) = &search.uses {
        query
            .push(
                " AND EXISTS (SELECT 1 FROM clips
                               WHERE clips.score_id = score.id AND clips.name LIKE ",
            )
            .push_bind(contains(uses))
            .push(" ESCAPE '\\')");
    }
    query.push(" ORDER BY ");
    if let Some(first) = &search.first {
        query.push(SAME_AUDIO).push_bind(first).push(") DESC, ");
    }
    query
        .push("datetime(edited_at) DESC, score.rowid DESC LIMIT ")
        .push_bind(i64::from(limit));
    query
        .build_query_as::<FoundScore>()
        .fetch_all(pool)
        .await
        .map_err(|error| format!("Failed to search scores: {error}"))
}

/// Fetch a score by ID
pub async fn get_score(access: &mut impl AuthorizedVenue, id: &str) -> Result<Score, String> {
    sqlx::query_as::<_, Score>(
        "SELECT id, uid, track_id, venue_id, name, grid_id, uid IS ? AS editable,
                created_at, updated_at
         FROM scores WHERE id = ? AND venue_id = ?",
    )
    .bind(access.principal().to_owned())
    .bind(id)
    .bind(access.venue_id().to_owned())
    .fetch_one(&mut *access.connection())
    .await
    .map_err(|e| format!("Failed to fetch score: {}", e))
}

/// Give a score a new name. A name equal to the stored one writes nothing, so
/// it uploads nothing either.
pub async fn rename_score(
    access: &mut VenueAccess<'_, Write>,
    id: &str,
    name: &str,
) -> Result<(), String> {
    sqlx::query("UPDATE scores SET name = ? WHERE id = ? AND venue_id = ? AND name IS NOT ?")
        .bind(name)
        .bind(id)
        .bind(access.venue_id().to_owned())
        .bind(name)
        .execute(&mut *access.connection())
        .await
        .map(|_| ())
        .map_err(|e| format!("Failed to rename score: {}", e))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

    pub(crate) async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let directory = tempfile::tempdir().expect("tempdir");
        let database_path = directory.path().join("luma-test.db");
        let migrate_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&database_path)
                    .journal_mode(SqliteJournalMode::Wal)
                    .create_if_missing(true)
                    .foreign_keys(false),
            )
            .await
            .expect("migration pool");
        sqlx::migrate!("./migrations")
            .run(&migrate_pool)
            .await
            .expect("migrations");
        migrate_pool.close().await;
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(database_path)
                    .journal_mode(SqliteJournalMode::Wal)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .expect("pool");
        (directory, pool)
    }

    /// Newest-created leads, and it is the *instant* that decides — not the
    /// text.
    ///
    /// The regression: `updated_at` used to be the key, and the two ways a
    /// timestamp is written disagree lexically. A local insert takes
    /// SQLite's `CURRENT_TIMESTAMP` (`2026-01-01 00:00:00`); the trigger that
    /// bumps `updated_at` uses RFC 3339 (`2026-01-01T00:00:00Z`). `'T' > ' '`,
    /// so *any* edited score sorted above *every* freshly made one and a new
    /// score never appeared at the top of the sidebar.
    #[tokio::test]
    async fn a_new_score_leads_the_listing_however_its_timestamp_is_spelled() {
        let (_directory, pool) = test_pool().await;
        sqlx::query("INSERT INTO tracks (id, uid, file_path, title, track_hash) VALUES ('track', 'alice', '/t.wav', 'T', 'hash')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO venues (id, uid, name) VALUES ('venue', 'alice', 'Venue')")
            .execute(&pool)
            .await
            .unwrap();
        // `oldest` is written the new way and later *edited*, so under the old
        // ordering it held the top slot for good; `newest` is a plain local
        // insert. `tie` is minted in the same second as `newest` — the
        // double-click case, which only `rowid` can order.
        for (id, name, created) in [
            ("oldest", "Oldest", "2026-01-01T00:00:00Z"),
            ("middle", "Middle", "2026-01-02 00:00:00"),
            ("newest", "Newest", "2026-01-03 00:00:00"),
            ("tie", "Tie", "2026-01-03 00:00:00"),
        ] {
            sqlx::query(
                "INSERT INTO scores (id, uid, track_id, venue_id, name, created_at, updated_at)
                 VALUES (?, 'alice', 'track', 'venue', ?, ?, ?)",
            )
            .bind(id)
            .bind(name)
            .bind(created)
            .bind(created)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("UPDATE scores SET updated_at = '2026-06-01T00:00:00Z' WHERE id = 'oldest'")
            .execute(&pool)
            .await
            .unwrap();

        crate::database::local::auth::arm_write_admission(&pool, Some("alice"))
            .await
            .unwrap();
        let listed = list_accessible_scores_for_track(&pool, "track")
            .await
            .expect("the listing");

        let order: Vec<&str> = listed.iter().map(|score| score.id.as_str()).collect();
        assert_eq!(order, ["tie", "newest", "middle", "oldest"]);
        // The list is the ordinal ladder upside down — `#3` can never sit
        // below `#2`, whichever way the two rows spell their timestamps.
        let ordinals: Vec<i64> = listed.iter().map(|score| score.ordinal).collect();
        assert_eq!(ordinals, [4, 3, 2, 1]);
    }

    /// Alice's library: one song lit in two rooms, another song in the first
    /// room, a score of Bob's in her room, and one in a room she cannot open.
    async fn library() -> (tempfile::TempDir, SqlitePool) {
        let (directory, pool) = test_pool().await;
        sqlx::query(
            "INSERT INTO tracks (id, uid, file_path, title, artist, track_hash) VALUES
                ('song', 'alice', '/a.wav', 'Night Drive', 'Kavinsky', 'hash-a'),
                ('song-again', 'alice', '/a2.wav', 'Night Drive (copy)', 'Kavinsky', 'hash-a'),
                ('other', 'alice', '/b.wav', 'Sunrise', 'Bonobo', 'hash-b');
             INSERT INTO venues (id, uid, name) VALUES
                ('gasworks', 'alice', 'Gasworks'),
                ('attic', 'alice', 'The 100% Attic'),
                ('bobs', 'bob', 'Bob''s Bar');
             INSERT INTO scores (id, uid, track_id, venue_id, name, authored_at) VALUES
                ('here', 'alice', 'song', 'attic', 'Attic show', '2026-03-01T00:00:00Z'),
                ('there', 'alice', 'song', 'gasworks', 'Main show', '2026-02-01T00:00:00Z'),
                ('copy', 'alice', 'song-again', 'gasworks', 'Alt', '2026-01-01T00:00:00Z'),
                ('sunrise', 'alice', 'other', 'gasworks', 'Warm', '2026-04-01T00:00:00Z'),
                ('guest', 'bob', 'song', 'gasworks', 'Bob in her room', '2026-06-01T00:00:00Z'),
                ('sealed', 'bob', 'song', 'bobs', 'Bob show', '2026-05-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (id, score, name) in [
            ("there:1", "there", "Kick chase"),
            ("there:2", "there", "Wash"),
            ("sunrise:1", "sunrise", "Wash"),
        ] {
            sqlx::query(
                "INSERT INTO clips (id, uid, score_id, name, graph, start, duration, seed,
                                    selection_json, blend_mode)
                 VALUES (?, 'alice', ?, ?, '', 0, 4, '1', '{}', 'replace')",
            )
            .bind(id)
            .bind(score)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
        }
        crate::database::local::auth::arm_write_admission(&pool, Some("alice"))
            .await
            .unwrap();
        (directory, pool)
    }

    async fn found(pool: &SqlitePool, search: ScoreSearch) -> Vec<String> {
        find_own_scores(pool, &search, 20)
            .await
            .expect("the search")
            .into_iter()
            .map(|score| score.id)
            .collect()
    }

    #[tokio::test]
    async fn a_search_lists_the_persons_own_scores_last_written_first() {
        let (_directory, pool) = library().await;
        assert_eq!(
            found(&pool, ScoreSearch::default()).await,
            ["sunrise", "here", "there", "copy"],
            "someone else's score was listed, or the order is not by last write"
        );
        let listed = find_own_scores(&pool, &ScoreSearch::default(), 20)
            .await
            .unwrap();
        let there = listed.iter().find(|score| score.id == "there").unwrap();
        assert_eq!((there.clips, there.venue_name.as_str()), (2, "Gasworks"));
    }

    #[tokio::test]
    async fn each_filter_narrows_the_search() {
        let (_directory, pool) = library().await;
        let search = |edit: fn(&mut ScoreSearch)| {
            let mut search = ScoreSearch::default();
            edit(&mut search);
            search
        };
        // The same audio under another track row is the same song.
        assert_eq!(
            found(
                &pool,
                search(|s| {
                    s.track = Some("song".into());
                    s.except = Some("here".into());
                })
            )
            .await,
            ["there", "copy"]
        );
        assert_eq!(
            found(&pool, search(|s| s.venue = Some("gas".into()))).await,
            ["sunrise", "there", "copy"]
        );
        // Every word must match somewhere, in any of the four names.
        assert_eq!(
            found(&pool, search(|s| s.text = Some("kavinsky MAIN".into()))).await,
            ["there"]
        );
        assert_eq!(
            found(&pool, search(|s| s.uses = Some("kick".into()))).await,
            ["there"]
        );
        // A wildcard in a name is a character, not a pattern.
        assert_eq!(
            found(&pool, search(|s| s.text = Some("100%".into()))).await,
            ["here"]
        );
        // One song's scores lead, and the rest follow by last write.
        assert_eq!(
            found(&pool, search(|s| s.first = Some("song".into()))).await,
            ["here", "there", "copy", "sunrise"]
        );
    }
}
