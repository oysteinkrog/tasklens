//! "tasklens Public IP Notice" and the public IP fetch entry point (UI-SHL-096, APP-056,
//! PERF-060, R-058, UI-FND-021).
//!
//! Every public IP fetch goes through [`request_public_ip_fetch`]. With
//! `network.public_ip_notice_accepted` it runs the fetch at once. Otherwise it opens the notice,
//! a `TlDialog` modal to the caller's window keyed `WindowKey::PublicIpNotice`: Continue writes
//! the key when "Do not ask again" is checked and then fetches; Cancel and Esc fetch nothing.
//! While the notice is open, another request focuses it and drops its own fetch.
//!
//! The fetch itself is the caller's (the Network page and the Information tab). This module makes
//! no network contact.

use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext as _, IntoElement, ParentElement as _,
    Styled as _, Window, div,
};
use tracing::warn;

use super::registry::{GlobalRegistry, WindowKey};
use crate::components::dialog::{DialogIcon, DialogResult, TlDialog};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::text::style::t_base;
use crate::theme::{current, metrics};

const TITLE: &str = "shell-public-ip-title";
const LINE_1: &str = "shell-public-ip-line-1";
const LINE_2: &str = "shell-public-ip-line-2";
const LINE_3: &str = "shell-public-ip-line-3";
const DONT_ASK: &str = "shell-public-ip-dont-ask";
const CONTINUE: &str = "shell-public-ip-continue";
const CANCEL: &str = "shell-confirm-cancel";

/// Runs `fetch` at once when the public IP notice was accepted, else asks first. `window` is
/// the caller's window; the notice is modal to it.
pub fn request_public_ip_fetch(
    window: &mut Window,
    cx: &mut App,
    fetch: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    if TlSettings::settings(cx).network.public_ip_notice_accepted {
        fetch(window, cx);
        return;
    }
    let parent = window.window_handle();
    // Deferred: opening the notice needs the parent window, which is mid-update here.
    cx.defer(move |cx| {
        let open = move |cx: &mut App| open_notice(parent, fetch, cx);
        match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
            Some(registry) => {
                registry.update(cx, |r, cx| r.open_or_focus(WindowKey::PublicIpNotice, cx, open));
            }
            None => {
                if let Err(e) = open(cx) {
                    warn!(target: "tasklens::ui", %e, "the public IP notice did not open");
                }
            }
        }
    });
}

/// Opens the notice on `parent` and returns its window.
fn open_notice(
    parent: AnyWindowHandle,
    fetch: impl FnOnce(&mut Window, &mut App) + 'static,
    cx: &mut App,
) -> gpui_kit::Result<AnyWindowHandle> {
    let dialog = TlDialog::confirm(tr(cx, LINE_1), tr(cx, CONTINUE))
        .title(tr(cx, TITLE))
        .icon(DialogIcon::None)
        .cancel_label(tr(cx, CANCEL))
        .extra(|_, cx| lines(cx))
        .checkbox(tr(cx, DONT_ASK), false);
    let before: Vec<AnyWindowHandle> = cx.windows();
    parent.update(cx, |_, window, cx| {
        dialog.open(window, cx, move |result, window, cx| {
            let DialogResult::Confirmed { checkbox } = result else {
                return;
            };
            if checkbox {
                TlSettings::set(
                    cx,
                    SettingKey::NetworkPublicIpNoticeAccepted,
                    SettingValue::Bool(true),
                );
            }
            fetch(window, cx);
        })
    })??;
    cx.windows()
        .into_iter()
        .find(|w| !before.iter().any(|b| b.window_id() == w.window_id()))
        .ok_or_else(|| gpui_kit::anyhow!("the public IP notice window is not listed"))
}

/// Lines 2 and 3, each its own paragraph in `t_base`.
fn lines(cx: &mut App) -> AnyElement {
    let theme = current(cx).clone();
    let m = *metrics(cx);
    let mut out = div().flex().flex_col().gap(m.sp[4]);
    for key in [LINE_2, LINE_3] {
        let mut line = div().text_color(theme.text_primary).child(tr(cx, key));
        *line.text_style() = t_base(&m);
        out = out.child(line);
    }
    out.into_any_element()
}
