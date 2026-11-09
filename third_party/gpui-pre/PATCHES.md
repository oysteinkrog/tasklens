# Local patches to gpui-pre and gpui-pre-wgpu 0.3.7

Source: crates.io gpui-pre 0.3.7 (.crate sha256 `0e87a42bb37c7cb4e76dd1ac0ce88851e46e976e0373a47ab3e0757abffee54d`). See D-043 in docs/DECISIONS.md.

| Change | Bead | Files | Commit |
|---|---|---|---|
| Paths are stored once per paint. `Scene::insert_primitive` moves a path into `Scene::paths` and `paint_operations` keeps only its id (no clone). `Scene::replay` clones a cached path once, not twice (`path_slots` maps the id to its sorted slot). `Window::paint_path` scales with the new `Path::into_scaled`, which reuses the vertex buffer. Test-only counter `scene_path_vertex_copies`. | tasklens-wlf.45 | `src/scene.rs`, `src/window.rs` | `ee146c4e1` |
| A text element's measure function shapes its text less often. It reuses its layout when the wrap and truncate widths match, and reuses an unwrapped layout of the whole text for any call that truncation would not cut (no truncate width, or a width the whole text fits, with no line clamp). When the untruncated lines fit and are also the final lines, they are kept instead of shaped again. The line wrapper is made only when truncation needs one. `WindowTextSystem::shape_text` sizes each line's decoration runs to the run count instead of 32. Output is the same. Test-only counter `text_measure_shapes`: two table frames with 27 rows went from 290 shapes to 58. | tasklens-wlf.46 | `src/elements/text.rs`, `src/text_system.rs` | `9c20b8156` |

## gpui-pre-wgpu 0.3.7

Source: crates.io gpui-pre-wgpu 0.3.7 (.crate sha256 `f0b02657b56b09140ce542f5e4434fb96d9fdaba0fd0035ba7dba13c171ddb9b`), in third_party/gpui-pre-wgpu. Same rules as above (D-043).

| Change | Bead | Files | Commit |
|---|---|---|---|
| A path vertex no longer carries its own colour and clip bounds. `draw_paths_to_intermediate` writes one 88-byte `PathRasterizationPath` record (colour and clipped bounds) per path, then 24-byte vertices (position, st and the record index) instead of 104-byte ones. The path rasterization pipeline binds the records as a second instance allocation at `@group(2)`; on the WebGL texture path the vertex holds the record's absolute index and the shader reads it from the same instance texture. `reserve_instance_data` grows the instance data before the two writes so both land in one buffer or texture. The fragment shader gets the record index as a flat varying in place of the vertex index. Shader output is the same. The record and vertex vectors are reused across frames. Bytes through `Queue::write_buffer` in the `static` bench scene went from 21.37 MB to 4.53 MB per frame (bpftrace memcpy probe as in tasklens-3uosp n2, 2026-11-09). | tasklens-3z9l1 | `src/wgpu_renderer.rs`, `src/shaders.wgsl`, `src/shaders_storage.wgsl`, `src/shaders_webgl.wgsl` | `9d8e39fcf` |
