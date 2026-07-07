//! Change-detection guard patterns (AGENTS.md §3.A #3).
//!
//! Prefer `DetectChangesMut::set_if_neq` (or the manual `if old != new` guard)
//! over unconditionally assigning through `&mut`, which would mark the component
//! changed every frame. Use `bypass_change_detection` only for write-only uploads
//! (e.g. GPU extract) where no consumer observes the change.

/// Assigns `new` to `current` only when it differs, returning whether a change
/// occurred. Equivalent to the built-in `Mut::set_if_neq` for non-ECS values.
///
/// # Example
/// ```
/// use strata_core::change_detection::assign_guarded;
/// let mut v = 5u32;
/// assert!(!assign_guarded(&mut v, 5)); // unchanged -> no flag
/// assert!(assign_guarded(&mut v, 6));  // changed
/// ```
pub fn assign_guarded<T: PartialEq + Clone>(current: &mut T, new: T) -> bool {
    if *current != new {
        *current = new;
        true
    } else {
        false
    }
}
