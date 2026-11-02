#!/usr/bin/env bash
# CS-D2 (tasklens-zljr5.13): rewrite tl-ui and tl-app imports of the UI settings and layout
# modules from tl-model to tl-ui-settings.
#
#   tl_model::settings      -> tl_ui_settings::settings
#   tl_model::layout        -> tl_ui_settings::layout
#   tl_model::layout_boxes  -> tl_ui_settings::layout_boxes
#   tl_model::layout_pages  -> tl_ui_settings::layout_pages
#
# tl_ui_settings::settings re-exports the engine settings types, so engine types such as
# EngineSettings move to the same path. tl-engine, tl-server and tl-remote are not touched: they
# keep tl_model::settings for the engine types.
#
# Group forms such as `use tl_model::{settings, layout}` are not rewritten. The script lists
# any it finds at the end; fix those by hand.
#
# Run from the repository root: bash scripts/split/settings_paths.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

dirs=(crates/tl-ui crates/tl-app)
pattern='tl_model::(settings|layout|layout_boxes|layout_pages)\b'

mapfile -t files < <(rg -l --type rust "$pattern" "${dirs[@]}" || true)
if ((${#files[@]} > 0)); then
    perl -pi -e 's/\btl_model::(settings|layout|layout_boxes|layout_pages)\b/tl_ui_settings::$1/g' "${files[@]}"
fi
echo "rewrote ${#files[@]} files"

echo "left to check by hand (group imports from tl_model naming a moved module):"
rg -n -U --type rust 'tl_model::\{[^}]*\b(settings|layout|layout_boxes|layout_pages)\b' "${dirs[@]}" || echo "  none"
