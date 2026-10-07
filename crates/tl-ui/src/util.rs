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
}
