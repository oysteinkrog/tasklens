//! The Performance tab's Overview page (UI-PRF-017, D-022): five btop-style boxes (CPU, MEM,
//! GPU, DISK, NET) in the tasklens theme, laid out from data so a later editor can change them.
//!
//! [`layout`] holds the layout model and its placement, [`parts`] the meter, core bars and box
//! frame. The page view and the box contents follow in later beads.

pub mod layout;
pub mod parts;
