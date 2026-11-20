//! `luma.scores`: the user's other scores, read-only.
//!
//! A track thread edits one score. To adapt a show to another rig, or to
//! learn from one, it also needs to read the others. Python names a
//! reference by score id and nothing else; its track and venue come off the
//! score's row, and a score that belongs to someone else is refused even in a
//! venue the user may open.
//!
//! A reference is read through the same hosts the thread's own score uses,
//! built for the reference's scope with no edit scope, no draft and no actor,
//! and reached only through the methods in [`READS`]. So a reference renders
//! and describes its room exactly as the current score does, and has no path
//! to a write.
use super::*;
use crate::agent_execution::spoken_groups::SpokenGroups;
use crate::agent_execution::venue_host::VenueHost;
use crate::database::local::scores::{self, rows, ScoreSearch};
use crate::database::local::venue_access::{AuthorizedVenue, Read, VenueAccess, VenueResource};

/// The track and venue methods a reference answers. Each one reads.
const READS: [&str; 8] = [
    "track.score_render",
    "venue.render",
    "venue.tiles",
    "venue.describe",
    "venue.groups",
    "venue.fixtures",
    "venue.query",
    "venue.extent",
];

const FIND_DEFAULT: u32 = 20;
const FIND_MAX: u32 = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindRequest {
    /// Words to find in the score, track, artist or venue name.
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    track_id: Option<String>,
    /// A venue id, or part of a venue's name.
    #[serde(default)]
    venue: Option<String>,
    /// Part of the name of a clip the score holds.
    #[serde(default)]
    uses: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenRequest {
    id: String,
}

/// One read of a reference: a track or venue method, answered for that score.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    score: String,
    method: String,
    #[serde(default)]
    payload: Value,
}

/// Refuse any method a reference does not answer, before its score is looked
/// up: what a reference may do does not depend on which reference it is.
fn admit(method: &str) -> Result<(), HostCallError> {
    if READS.contains(&method) {
        return Ok(());
    }
    Err(HostCallError::new(
        "forbidden",
        format!(
            "a reference score and its venue are read-only, and {method} is not a read; \
             change the current score with luma.track.edit()"
        ),
    ))
}

fn forbidden(error: String) -> HostCallError {
    HostCallError::new("forbidden", error)
}

fn internal(error: sqlx::Error) -> HostCallError {
    HostCallError::new("internal", error.to_string())
}

impl TrackHost {
    pub(super) async fn scores_call(
        &self,
        method: &str,
        payload: Value,
    ) -> Result<Value, HostCallError> {
        match method {
            "track.scores_find" => self.scores_find(decode(payload)?).await,
            _ => self.scores_open(decode(payload)?).await,
        }
    }

    /// The user's own scores in venues they can open, this track's first and
    /// then the newest. The score this thread edits is left out.
    async fn scores_find(&self, request: FindRequest) -> Result<Value, HostCallError> {
        let search = ScoreSearch {
            text: request.text,
            track: request.track_id,
            venue: request.venue,
            uses: request.uses,
            except: Some(self.scope.score_id.clone()),
            first: Some(self.scope.track_id.clone()),
        };
        let limit = request.limit.unwrap_or(FIND_DEFAULT).clamp(1, FIND_MAX);
        // One more than asked for says whether the list was cut.
        let mut found = scores::find_own_scores(&self.pool, &search, limit + 1)
            .await
            .map_err(|error| HostCallError::new("internal", error))?;
        let more = found.len() > limit as usize;
        found.truncate(limit as usize);
        Ok(json!({ "scores": found, "more": more }))
    }

    /// A read guard on one of the user's own scores, and the scope it is read
    /// in, off its row.
    async fn reference(
        &self,
        score_id: &str,
    ) -> Result<(VenueAccess<'_, Read>, TrackScope, Option<String>), HostCallError> {
        let mut access = VenueAccess::<Read>::read(&self.pool, VenueResource::Score(score_id))
            .await
            .map_err(forbidden)?;
        let row: Option<(Option<String>, String, Option<String>)> =
            sqlx::query_as("SELECT uid, track_id, name FROM scores WHERE id = ?")
                .bind(score_id)
                .fetch_optional(&mut *access.connection())
                .await
                .map_err(internal)?;
        let Some((uid, track_id, name)) = row else {
            return Err(HostCallError::new("not_found", "no such score"));
        };
        if uid.as_deref() != Some(access.principal()) {
            return Err(HostCallError::new(
                "forbidden",
                "luma.scores reads only your own scores",
            ));
        }
        let scope = TrackScope {
            score_id: score_id.to_owned(),
            track_id,
            venue_id: access.venue_id().to_owned(),
        };
        Ok((access, scope, name))
    }

    /// One method of [`READS`], answered by a track and a venue host built for
    /// the reference. Called outside this host's own runtime block: each host
    /// blocks on the runtime itself.
    pub(super) fn scores_read(
        &self,
        payload: Value,
        context: &HostCallContext,
    ) -> Result<Value, HostCallError> {
        let request: ReadRequest = decode(payload)?;
        admit(&request.method)?;
        let limit = call_limit(context)?;
        let scope = self.runtime.block_on(supervise(
            async { Ok(self.reference(&request.score).await?.1) },
            context,
            limit,
        ))?;
        let venue_id = scope.venue_id.clone();
        let track = Arc::new(TrackHost::new(
            self.runtime.clone(),
            self.pool.clone(),
            self.storage.clone(),
            self.resource_root.clone(),
            Arc::clone(&self.controls),
            Arc::clone(&self.workspace),
            scope,
            None,
            None,
            TurnAttribution::default(),
        ));
        if request.method.starts_with("track.") {
            return track.handle(&request.method, request.payload, context);
        }
        VenueHost::new(
            self.runtime.clone(),
            self.pool.clone(),
            self.resource_root.clone(),
            Arc::clone(&self.workspace),
            venue_id,
            TurnAttribution::default(),
            Some(track),
        )
        .handle(&request.method, request.payload, context)
    }

    /// One of the user's scores as Python holds it: its document with groups
    /// named, its own track's clock, and what each group of its venue is made
    /// of. The room itself is read live, per call.
    async fn scores_open(&self, request: OpenRequest) -> Result<Value, HostCallError> {
        let (mut access, scope, name) = self.reference(&request.id).await?;
        let venue_id = scope.venue_id.clone();
        let venue_name: Option<String> = sqlx::query_scalar("SELECT name FROM venues WHERE id = ?")
            .bind(&venue_id)
            .fetch_optional(&mut *access.connection())
            .await
            .map_err(internal)?;
        let score = rows::load_score(access.connection(), &request.id)
            .await
            .map_err(|error| HostCallError::new("invalid_score", error))?;
        let spoken = SpokenGroups::read(&self.resource_root, &mut access)
            .await
            .map_err(|error| HostCallError::new("internal", error))?;
        let groups: Vec<(Option<String>, i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT grp.name, COUNT(DISTINCT member.fixture_id), COUNT(member.head_index),
                    GROUP_CONCAT(DISTINCT fixture.model)
             FROM fixture_groups grp
             LEFT JOIN fixture_group_members member ON member.group_id = grp.id
             LEFT JOIN fixtures fixture ON fixture.id = member.fixture_id
             WHERE grp.venue_id = ?
             GROUP BY grp.id
             ORDER BY grp.display_order, grp.name",
        )
        .bind(&venue_id)
        .fetch_all(&mut *access.connection())
        .await
        .map_err(internal)?;
        let groups: Vec<Value> = groups
            .into_iter()
            .map(|(name, fixtures, cells, models)| {
                let models: Vec<&str> = models
                    .as_deref()
                    .map(|models| models.split(',').collect())
                    .unwrap_or_default();
                json!({ "name": name, "fixtures": fixtures, "cells": cells, "models": models })
            })
            .collect();
        drop(access);
        let unavailable = |error: String| HostCallError::new("internal", error);
        let track = crate::database::local::tracks::get_track_by_id(&self.pool, &scope.track_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| HostCallError::new("not_found", "the score's track is not available"))?;
        let grid = crate::database::local::tracks::get_track_beats(&self.pool, scope.grid())
            .await
            .map_err(unavailable)?;
        Ok(json!({
            "id": request.id,
            "name": name,
            "same_track": scope.track_id == self.scope.track_id,
            "track": {
                "id": track.id,
                "title": track.title,
                "artist": track.artist,
                "album": track.album,
                "duration_s": track.duration_seconds,
                "bpm": grid.as_ref().map(|grid| grid.bpm),
                "beat_origin_s": grid.as_ref().map(|grid| {
                    grid.downbeats.first().copied().unwrap_or(grid.downbeat_offset)
                }),
                "document": spoken.spoken(json!(score)),
                "editable": false,
            },
            "features": {
                "beats": grid.as_ref().map(|grid| grid.beats.clone()),
                "downbeats": grid.as_ref().map(|grid| grid.downbeats.clone()),
                "beats_per_bar": grid.as_ref().map(|grid| grid.beats_per_bar),
            },
            "venue": {
                "id": venue_id,
                "name": venue_name,
                "views": luma_scene::View::ALL.iter().map(|view| view.name()).collect::<Vec<_>>(),
                "groups": groups,
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference renders and describes; it never applies, builds or patches.
    /// A verb added to a track or venue host is refused here until it is
    /// listed as a read.
    #[test]
    fn a_reference_answers_reads_and_refuses_everything_else() {
        for read in ["track.score_render", "venue.render", "venue.groups"] {
            assert!(admit(read).is_ok(), "{read} was refused");
        }
        for write in [
            "track.score_apply",
            "track.scores_read",
            "venue.place",
            "venue.group",
            "venue.remove",
            "venue.patch",
            "venue.environment",
            "venue.haze",
            "venue.stamp",
            "perform.add",
            "venue.a_verb_added_later",
        ] {
            let refused = admit(write).expect_err(write);
            assert_eq!(refused.code, "forbidden", "{write}");
        }
    }
}
