//! Small helpers shared by several views.

use gpui_kit::{App, ClipboardItem};

/// Writes `text` to the clipboard. Every copy command goes through here and shows its success
/// toast on `Ok`. `App::write_to_clipboard` returns `()` in gpui-pre 0.3.7, so no failure can
/// be detected and this always returns `Ok(())`. The failure texts stay in Fluent, unused in v1
/// (R-199).
#[allow(clippy::result_unit_err)]
pub fn clipboard_write(cx: &mut App, text: String) -> Result<(), ()> {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    Ok(())
}

/// `path` with a leading `home` shown as `~`, or `None` when it does not start with `home`.
pub fn home_tilde(path: &str, home: &str) -> Option<String> {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return None;
    }
    let rest = path.strip_prefix(home)?;
    (rest.is_empty() || rest.starts_with('/')).then(|| format!("~{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn clipboard_write_ok(cx: &mut TestAppContext) {
        let result = cx.update(|cx| clipboard_write(cx, "abc".into()));
        assert_eq!(result, Ok(()));
        let text = cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
        assert_eq!(text.as_deref(), Some("abc"));
    }

    #[test]
    fn tilde_only_at_a_path_boundary() {
        assert_eq!(
            home_tilde("/home/u/.config/autostart", "/home/u").as_deref(),
            Some("~/.config/autostart")
        );
        assert_eq!(home_tilde("/home/u", "/home/u/").as_deref(), Some("~"));
        assert_eq!(home_tilde("/home/user2/x", "/home/u"), None);
        assert_eq!(home_tilde("/etc/xdg/autostart", "/home/u"), None);
        assert_eq!(home_tilde("/etc", ""), None);
    }
}
