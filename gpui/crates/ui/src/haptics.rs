//! Discrete native feedback for user-driven control detents.
//!
//! Adapted from Zeron (`crates/ui/src/haptics.rs`), MIT, (c) 2026 Wing.

/// One detent's tap on a haptic trackpad: AppKit's level change, performed
/// now. Nothing happens on another platform, or on a Mac without one.
pub fn selection_step() {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use objc2_app_kit::{
            NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime,
            NSHapticFeedbackPerformer,
        };
        // Input handlers run on the main thread, where AppKit's performer
        // lives.
        if objc2::MainThreadMarker::new().is_some() {
            NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(
                NSHapticFeedbackPattern::LevelChange,
                NSHapticFeedbackPerformanceTime::Now,
            );
        }
    }
    #[cfg(test)]
    STEPS.with(|steps| steps.set(steps.get() + 1));
}

#[cfg(test)]
thread_local! { static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// How many taps this thread has asked for, for a test of a control.
#[cfg(test)]
pub fn step_count() -> usize {
    STEPS.with(|steps| steps.get())
}
