# Local patches to gpui-pre 0.3.7

Source: crates.io gpui-pre 0.3.7 (.crate sha256 `0e87a42bb37c7cb4e76dd1ac0ce88851e46e976e0373a47ab3e0757abffee54d`). See D-043 in docs/DECISIONS.md.

| Change | Bead | Files | Commit |
|---|---|---|---|
| Paths are stored once per paint. `Scene::insert_primitive` moves a path into `Scene::paths` and `paint_operations` keeps only its id (no clone). `Scene::replay` clones a cached path once, not twice (`path_slots` maps the id to its sorted slot). `Window::paint_path` scales with the new `Path::into_scaled`, which reuses the vertex buffer. Test-only counter `scene_path_vertex_copies`. | tasklens-wlf.45 | `src/scene.rs`, `src/window.rs` | pending |
