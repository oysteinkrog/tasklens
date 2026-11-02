//! The toast data types every surface pushes with: kind, key and action button (UI-FND-020,
//! R-113, R-197). The queue view that shows them is `components::toast::ToastQueue`, which
//! re-exports these types. They sit here, below the kit, so the base layer (the action tracker,
//! `ShellApi`) can name them (ADR-020, CS-C6).

use gpui_kit::{Action, SharedString};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
    /// An action in flight. Never auto-dismissed and never takes one of the 4 slots (R-197).
    Progress,
}

impl ToastKind {
    /// The kind spelled out in English, for logs.
    pub fn name(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Success => "Success",
            Self::Warning => "Warning",
            Self::Error => "Error",
            Self::Progress => "Progress",
        }
    }
}

/// An owner's toast identity. Pushing again with the same key replaces the toast in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToastKey(pub u64);

/// An optional button under the text. It dispatches `action`, then closes the toast.
pub struct ToastAction {
    pub label: SharedString,
    pub action: Box<dyn Action>,
}

impl Clone for ToastAction {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            action: self.action.boxed_clone(),
        }
    }
}
