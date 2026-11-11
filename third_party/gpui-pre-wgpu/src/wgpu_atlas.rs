use anyhow::{Context as _, Result};
use etagere::{BucketedAtlasAllocator, size2};
use gpui::{
    AtlasBackend, AtlasKey, AtlasState, AtlasTextureId, AtlasTextureKind, AtlasTextureList,
    AtlasTile, Bounds, DevicePixels, PlatformAtlas, Point, Size,
};
use parking_lot::Mutex;
use std::{
    borrow::Cow,
    ops,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::WgpuContext;

fn device_size_to_etagere(size: Size<DevicePixels>) -> etagere::Size {
    size2(size.width.0, size.height.0)
}

fn etagere_point_to_device(point: etagere::Point) -> Point<DevicePixels> {
    Point {
        x: DevicePixels(point.x),
        y: DevicePixels(point.y),
    }
}

pub struct WgpuAtlas(Mutex<AtlasState<WgpuAtlasTextures>>);

struct PendingUpload {
    id: AtlasTextureId,
    bounds: Bounds<DevicePixels>,
    data: Vec<u8>,
}

/// One tile upload, handed to the renderer, which copies it from its staging
/// buffer into `texture` with the frame's other uploads (tasklens-x12tr.1).
pub(crate) struct AtlasUpload {
    pub(crate) texture: wgpu::Texture,
    pub(crate) origin: wgpu::Origin3d,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bytes_per_pixel: u32,
    /// Tightly packed rows, `width * bytes_per_pixel` bytes each.
    pub(crate) data: Vec<u8>,
}

impl AtlasUpload {
    pub(crate) fn row_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.bytes_per_pixel)
    }

    /// Row pitch in the staging buffer. `copy_buffer_to_texture` needs rows on
    /// `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes); `Queue::write_texture` did not.
    pub(crate) fn padded_row_bytes(&self) -> u64 {
        self.row_bytes()
            .next_multiple_of(u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT))
    }

    pub(crate) fn staging_bytes(&self) -> u64 {
        self.padded_row_bytes() * u64::from(self.height)
    }
}

struct WgpuAtlasTextures {
    device: Arc<wgpu::Device>,
    max_texture_size: u32,
    color_texture_format: wgpu::TextureFormat,
    storage: WgpuAtlasStorage,
    pending_uploads: Vec<PendingUpload>,
    next_texture_generation: u64,
    /// Device textures made by `push_texture` so far. A spare reuse does not count.
    textures_created: u64,
}

/// A spare is dropped when it has not been reused for this long (tasklens-x12tr.2).
const SPARE_IDLE_RELEASE: Duration = Duration::from_secs(60);

pub struct WgpuTextureInfo {
    pub view: wgpu::TextureView,
    /// Distinguishes new textures that reuse an [`AtlasTextureId`].
    pub generation: u64,
}

impl WgpuAtlas {
    /// `_queue` is unused since the renderer uploads tiles through its own staging
    /// buffers (tasklens-x12tr.1). It stays so the signature matches upstream.
    pub fn new(
        device: Arc<wgpu::Device>,
        _queue: Arc<wgpu::Queue>,
        color_texture_format: wgpu::TextureFormat,
    ) -> Self {
        let max_texture_size = device.limits().max_texture_dimension_2d;
        WgpuAtlas(Mutex::new(AtlasState::new(WgpuAtlasTextures {
            device,
            max_texture_size,
            color_texture_format,
            storage: WgpuAtlasStorage::default(),
            pending_uploads: Vec::new(),
            next_texture_generation: 0,
            textures_created: 0,
        })))
    }

    pub fn from_context(context: &WgpuContext) -> Self {
        Self::new(
            context.device.clone(),
            context.queue.clone(),
            context.color_texture_format(),
        )
    }

    /// Moves the pending tile uploads into `out`, skipping those whose texture was
    /// removed. The renderer copies them in the next submit.
    pub(crate) fn drain_uploads(&self, out: &mut Vec<AtlasUpload>) {
        let mut lock = self.0.lock();
        lock.backend.drain_uploads(out);
    }

    /// Returns the view backing `id`, or `None` once every tile in it has been
    /// removed. A scene can still reference such a texture when a cached view
    /// replays a paint from before the image was dropped, so callers must skip
    /// those sprites rather than assume the texture exists.
    pub fn get_texture_info(&self, id: AtlasTextureId) -> Option<WgpuTextureInfo> {
        let lock = self.0.lock();
        let texture = lock.backend.storage.get(id)?;
        Some(WgpuTextureInfo {
            view: texture.view.clone(),
            generation: texture.generation,
        })
    }

    /// Clears all cached textures and tiles, forcing them to be recreated.
    /// Use this for incremental recovery when the device is still valid.
    pub fn clear(&self) {
        self.0.lock().clear(|textures| {
            textures.storage = WgpuAtlasStorage::default();
            textures.pending_uploads.clear();
        });
    }

    /// Handles device lost by clearing all textures and cached tiles.
    /// The atlas will lazily recreate textures as needed on subsequent frames.
    pub fn handle_device_lost(&self, context: &WgpuContext) {
        self.0.lock().clear(|textures| {
            textures.device = context.device.clone();
            textures.color_texture_format = context.color_texture_format();
            textures.storage = WgpuAtlasStorage::default();
            textures.pending_uploads.clear();
        });
    }
}

impl PlatformAtlas for WgpuAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        self.0.lock().get_or_insert_with(key, build)
    }

    fn remove(&self, key: &AtlasKey) {
        self.0.lock().remove(key);
    }
}

impl AtlasBackend for WgpuAtlasTextures {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        let tile = self.allocate(size, kind).context("failed to allocate")?;
        self.upload_texture(tile.texture_id, tile.bounds, bytes);
        Ok(tile)
    }

    fn remove(&mut self, tile: AtlasTile) {
        let id = tile.texture_id;
        let Some(texture_slot) = self.storage[id.kind].textures.get_mut(id.index as usize) else {
            return;
        };

        if let Some(mut texture) = texture_slot.take() {
            texture.allocator.deallocate(tile.tile_id.into());
            texture.decrement_ref_count();
            if texture.is_unreferenced() {
                self.pending_uploads
                    .retain(|upload| upload.id != texture.id);
                self.storage[id.kind]
                    .free_list
                    .push(texture.id.index as usize);
                let extent = texture.texture.size();
                self.storage.spares.push(SpareTexture {
                    kind: id.kind,
                    size: Size {
                        width: DevicePixels(extent.width as i32),
                        height: DevicePixels(extent.height as i32),
                    },
                    format: texture.format,
                    texture: texture.texture,
                    view: texture.view,
                    emptied_at: Instant::now(),
                });
            } else {
                *texture_slot = Some(texture);
            }
        }
    }
}

impl WgpuAtlasTextures {
    fn allocate(
        &mut self,
        size: Size<DevicePixels>,
        texture_kind: AtlasTextureKind,
    ) -> Option<AtlasTile> {
        {
            let textures = &mut self.storage[texture_kind];

            if let Some(tile) = textures
                .iter_mut()
                .rev()
                .find_map(|texture| texture.allocate(size))
            {
                return Some(tile);
            }
        }

        let texture = self.push_texture(size, texture_kind);
        texture.allocate(size)
    }

    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> &mut WgpuAtlasTexture {
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(1024),
            height: DevicePixels(1024),
        };
        let max_texture_size = self.max_texture_size as i32;
        let max_atlas_size = Size {
            width: DevicePixels(max_texture_size),
            height: DevicePixels(max_texture_size),
        };

        let size = min_size.min(&max_atlas_size).max(&DEFAULT_ATLAS_SIZE);
        let format = match kind {
            AtlasTextureKind::Monochrome => wgpu::TextureFormat::R8Unorm,
            AtlasTextureKind::Subpixel | AtlasTextureKind::Polychrome => self.color_texture_format,
        };

        let spare =
            self.storage.spares.iter().position(|spare| {
                spare.kind == kind && spare.size == size && spare.format == format
            });
        let (texture, view) = if let Some(ix) = spare {
            let spare = self.storage.spares.swap_remove(ix);
            (spare.texture, spare.view)
        } else {
            // No spare fits, so drop this kind's spares before creating. This
            // keeps the bound on `WgpuAtlasStorage::spares`: without it, textures
            // of different sizes inserted and removed one after the other would
            // pile up as spares while only one is ever live.
            self.storage.spares.retain(|spare| spare.kind != kind);
            self.textures_created += 1;
            self.create_texture(size, format)
        };

        let texture_list = &mut self.storage[kind];
        let index = texture_list.free_list.pop();
        let generation = self.next_texture_generation;
        self.next_texture_generation = self.next_texture_generation.wrapping_add(1);

        let atlas_texture = WgpuAtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(texture_list.textures.len()) as u32,
                kind,
            },
            generation,
            allocator: BucketedAtlasAllocator::new(device_size_to_etagere(size)),
            format,
            texture,
            view,
            live_atlas_keys: 0,
        };

        if let Some(ix) = index {
            texture_list.textures[ix] = Some(atlas_texture);
            texture_list
                .textures
                .get_mut(ix)
                .and_then(|t| t.as_mut())
                .expect("texture must exist")
        } else {
            texture_list.textures.push(Some(atlas_texture));
            texture_list
                .textures
                .last_mut()
                .and_then(|t| t.as_mut())
                .expect("texture must exist")
        }
    }

    fn create_texture(
        &self,
        size: Size<DevicePixels>,
        format: wgpu::TextureFormat,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d {
                width: size.width.0 as u32,
                height: size.height.0 as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    /// Drops the spares that have not been reused for `SPARE_IDLE_RELEASE`.
    fn release_idle_spares(&mut self, now: Instant) {
        self.storage
            .spares
            .retain(|spare| now.saturating_duration_since(spare.emptied_at) < SPARE_IDLE_RELEASE);
    }

    fn upload_texture(&mut self, id: AtlasTextureId, bounds: Bounds<DevicePixels>, bytes: &[u8]) {
        let data = self
            .storage
            .get(id)
            .map(|texture| swizzle_upload_data(bytes, texture.format))
            .unwrap_or_else(|| bytes.to_vec());

        self.pending_uploads
            .push(PendingUpload { id, bounds, data });
    }

    fn drain_uploads(&mut self, out: &mut Vec<AtlasUpload>) {
        self.release_idle_spares(Instant::now());
        for upload in self.pending_uploads.drain(..) {
            let Some(texture) = self.storage.get(upload.id) else {
                continue;
            };
            if upload.bounds.size.width.0 <= 0 || upload.bounds.size.height.0 <= 0 {
                continue;
            }
            out.push(AtlasUpload {
                texture: texture.texture.clone(),
                origin: wgpu::Origin3d {
                    x: upload.bounds.origin.x.0 as u32,
                    y: upload.bounds.origin.y.0 as u32,
                    z: 0,
                },
                width: upload.bounds.size.width.0 as u32,
                height: upload.bounds.size.height.0 as u32,
                bytes_per_pixel: u32::from(texture.bytes_per_pixel()),
                data: upload.data,
            });
        }
    }
}

#[derive(Default)]
struct WgpuAtlasStorage {
    monochrome_textures: AtlasTextureList<WgpuAtlasTexture>,
    subpixel_textures: AtlasTextureList<WgpuAtlasTexture>,
    polychrome_textures: AtlasTextureList<WgpuAtlasTexture>,
    /// Emptied textures kept for reuse, so a later insert of the same kind, size
    /// and format takes one instead of making a new device texture
    /// (tasklens-x12tr.2). Bound: the live plus spare textures of one kind never
    /// exceed the peak live count of that kind. Emptying a texture and reusing a
    /// spare each move one texture between live and spare, so the sum stays the
    /// same. `push_texture` creates a texture only when no spare of the kind
    /// fits, and then drops all spares of that kind first, so the sum becomes the
    /// live count. A spare can be a different size than the next request needs;
    /// it is then dropped, not kept. A spare is dropped when it has not been reused for `SPARE_IDLE_RELEASE`
    /// (60 s, checked in `drain_uploads`, once per drawn frame, so the check waits
    /// for the next frame when none is drawn), on `clear()` and on device loss,
    /// which replace the whole storage. Desktop platforms send no memory-pressure
    /// signal (`Platform::on_memory_warning` is never called), so there is no
    /// release on memory pressure.
    spares: Vec<SpareTexture>,
}

/// An emptied atlas texture. Its old pixels stay; new tiles overwrite only
/// their own bounds, the same as a tile reused inside a live texture.
struct SpareTexture {
    kind: AtlasTextureKind,
    size: Size<DevicePixels>,
    format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    emptied_at: Instant,
}

impl ops::Index<AtlasTextureKind> for WgpuAtlasStorage {
    type Output = AtlasTextureList<WgpuAtlasTexture>;
    fn index(&self, kind: AtlasTextureKind) -> &Self::Output {
        match kind {
            AtlasTextureKind::Monochrome => &self.monochrome_textures,
            AtlasTextureKind::Subpixel => &self.subpixel_textures,
            AtlasTextureKind::Polychrome => &self.polychrome_textures,
        }
    }
}

impl ops::IndexMut<AtlasTextureKind> for WgpuAtlasStorage {
    fn index_mut(&mut self, kind: AtlasTextureKind) -> &mut Self::Output {
        match kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Subpixel => &mut self.subpixel_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
        }
    }
}

impl WgpuAtlasStorage {
    fn get(&self, id: AtlasTextureId) -> Option<&WgpuAtlasTexture> {
        self[id.kind]
            .textures
            .get(id.index as usize)
            .and_then(|t| t.as_ref())
    }
}

struct WgpuAtlasTexture {
    id: AtlasTextureId,
    generation: u64,
    allocator: BucketedAtlasAllocator,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    live_atlas_keys: u32,
}

impl WgpuAtlasTexture {
    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let allocation = self.allocator.allocate(device_size_to_etagere(size))?;
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id: allocation.id.into(),
            padding: 0,
            bounds: Bounds {
                origin: etagere_point_to_device(allocation.rectangle.min),
                size,
            },
        };
        self.live_atlas_keys += 1;
        Some(tile)
    }

    fn bytes_per_pixel(&self) -> u8 {
        match self.format {
            wgpu::TextureFormat::R8Unorm => 1,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm => 4,
            _ => 4,
        }
    }

    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    fn is_unreferenced(&self) -> bool {
        self.live_atlas_keys == 0
    }
}

fn swizzle_upload_data(bytes: &[u8], format: wgpu::TextureFormat) -> Vec<u8> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => {
            let mut data = bytes.to_vec();
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            data
        }
        _ => bytes.to_vec(),
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use gpui::block_on;
    use gpui::{ImageId, RenderImageParams};
    use std::sync::Arc;

    fn test_device_and_queue() -> anyhow::Result<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
        block_on(async {
            // Vulkan only: without a display the GL adapter's device is lost at once.
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                flags: wgpu::InstanceFlags::default(),
                backend_options: wgpu::BackendOptions::default(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                display: None,
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                })
                .await
                .map_err(|error| anyhow::anyhow!("failed to request adapter: {error}"))?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("wgpu_atlas_test_device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits())
                        .using_alignment(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    trace: wgpu::Trace::Off,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                })
                .await
                .map_err(|error| anyhow::anyhow!("failed to request device: {error}"))?;
            Ok((Arc::new(device), Arc::new(queue)))
        })
    }

    #[test]
    fn drain_skips_uploads_for_removed_texture() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;

        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(1),
            frame_index: 0,
        });
        let size = Size {
            width: DevicePixels(1),
            height: DevicePixels(1),
        };
        let mut build = || Ok(Some((size, Cow::Owned(vec![0, 0, 0, 255]))));

        // Regression test: before the fix, this panicked in flush_uploads
        atlas
            .get_or_insert_with(key.clone(), &mut build)?
            .expect("tile should be created");
        atlas.remove(&key);
        let mut uploads = Vec::new();
        atlas.drain_uploads(&mut uploads);
        assert!(uploads.is_empty());
        Ok(())
    }

    #[test]
    fn drain_hands_over_live_uploads_once() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(1),
            frame_index: 0,
        });
        let size = Size {
            width: DevicePixels(3),
            height: DevicePixels(2),
        };
        let tile = atlas
            .get_or_insert_with(key, &mut || {
                Ok(Some((size, Cow::Owned(vec![7; 3 * 2 * 4]))))
            })?
            .expect("tile should be created");

        let mut uploads = Vec::new();
        atlas.drain_uploads(&mut uploads);
        assert_eq!(uploads.len(), 1);
        let upload = &uploads[0];
        assert_eq!(
            (upload.width, upload.height, upload.bytes_per_pixel),
            (3, 2, 4)
        );
        assert_eq!(upload.origin.x, tile.bounds.origin.x.0 as u32);
        assert_eq!(upload.origin.y, tile.bounds.origin.y.0 as u32);
        assert_eq!(upload.data, vec![7; 24]);
        assert_eq!(upload.row_bytes(), 12);
        assert_eq!(upload.padded_row_bytes(), 256);
        assert_eq!(upload.staging_bytes(), 512);

        uploads.clear();
        atlas.drain_uploads(&mut uploads);
        assert!(uploads.is_empty());
        Ok(())
    }

    #[test]
    fn remove_deallocates_tile_space_for_reuse() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);

        let small = Size {
            width: DevicePixels(64),
            height: DevicePixels(64),
        };
        let big = Size {
            width: DevicePixels(700),
            height: DevicePixels(700),
        };

        let make_key = |image_id: usize| {
            AtlasKey::Image(RenderImageParams {
                image_id: ImageId(image_id),
                frame_index: 0,
            })
        };
        let insert = |key: AtlasKey, size: Size<DevicePixels>| {
            let byte_count = (size.width.0 as usize) * (size.height.0 as usize) * 4;
            atlas
                .get_or_insert_with(key, &mut || {
                    Ok(Some((size, Cow::Owned(vec![0u8; byte_count]))))
                })
                .expect("allocation should succeed")
                .expect("callback returns Some")
        };

        let keeper_key = make_key(1);
        let big_key_a = make_key(2);
        let big_key_b = make_key(3);

        let keeper_tile = insert(keeper_key, small);
        let tile_a = insert(big_key_a.clone(), big);
        assert_eq!(keeper_tile.texture_id, tile_a.texture_id);

        atlas.remove(&big_key_a);
        let tile_b = insert(big_key_b, big);
        assert_eq!(tile_b.texture_id, keeper_tile.texture_id);
        Ok(())
    }

    #[test]
    fn reused_texture_id_has_new_generation() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let size = Size {
            width: DevicePixels(700),
            height: DevicePixels(700),
        };
        let make_key = |image_id| {
            AtlasKey::Image(RenderImageParams {
                image_id: ImageId(image_id),
                frame_index: 0,
            })
        };
        let insert = |key: AtlasKey| {
            atlas
                .get_or_insert_with(key, &mut || {
                    Ok(Some((
                        size,
                        Cow::Owned(vec![0; size.width.0 as usize * size.height.0 as usize * 4]),
                    )))
                })
                .expect("allocation should succeed")
                .expect("callback returns Some")
        };

        let first_key = make_key(1);
        let first_tile = insert(first_key.clone());
        let first_generation = atlas
            .get_texture_info(first_tile.texture_id)
            .context("first texture should exist")?
            .generation;
        atlas.remove(&first_key);
        assert!(atlas.get_texture_info(first_tile.texture_id).is_none());

        let second_tile = insert(make_key(2));
        let second_generation = atlas
            .get_texture_info(second_tile.texture_id)
            .context("second texture should exist")?
            .generation;

        assert_eq!(second_tile.texture_id, first_tile.texture_id);
        assert_ne!(second_generation, first_generation);
        Ok(())
    }

    fn image_key(image_id: usize) -> AtlasKey {
        AtlasKey::Image(RenderImageParams {
            image_id: ImageId(image_id),
            frame_index: 0,
        })
    }

    /// Inserts a 700x700 image. Two of them never share one 1024x1024 texture.
    fn insert_big(atlas: &WgpuAtlas, key: AtlasKey) -> AtlasTile {
        let size = Size {
            width: DevicePixels(700),
            height: DevicePixels(700),
        };
        atlas
            .get_or_insert_with(key, &mut || {
                Ok(Some((size, Cow::Owned(vec![0; 700 * 700 * 4]))))
            })
            .expect("allocation should succeed")
            .expect("callback returns Some")
    }

    fn textures_created(atlas: &WgpuAtlas) -> u64 {
        atlas.0.lock().backend.textures_created
    }

    fn spare_count(atlas: &WgpuAtlas) -> usize {
        atlas.0.lock().backend.storage.spares.len()
    }

    #[test]
    fn reinsert_after_remove_creates_no_texture() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let (key_a, key_b) = (image_key(1), image_key(2));

        let tile_a = insert_big(&atlas, key_a.clone());
        let tile_b = insert_big(&atlas, key_b.clone());
        assert_ne!(tile_a.texture_id, tile_b.texture_id);
        assert_eq!(textures_created(&atlas), 2);

        for _ in 0..100 {
            atlas.remove(&key_a);
            atlas.remove(&key_b);
            assert_eq!(spare_count(&atlas), 2);
            insert_big(&atlas, key_a.clone());
            insert_big(&atlas, key_b.clone());
            assert_eq!(spare_count(&atlas), 0);
        }
        assert_eq!(textures_created(&atlas), 2);
        Ok(())
    }

    #[test]
    fn spares_stay_within_peak_live_count() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);

        // Each image needs its own texture size (1024, 1500, 2100), and only one
        // is live at a time, so at most one spare may stay.
        for (round, side) in [700, 1500, 2100, 700, 1500].into_iter().enumerate() {
            let key = image_key(round + 1);
            let size = Size {
                width: DevicePixels(side),
                height: DevicePixels(side),
            };
            let bytes = vec![0; (side * side * 4) as usize];
            atlas
                .get_or_insert_with(key.clone(), &mut || {
                    Ok(Some((size, Cow::Borrowed(bytes.as_slice()))))
                })?
                .expect("callback returns Some");
            assert_eq!(spare_count(&atlas), 0);
            atlas.remove(&key);
            assert_eq!(spare_count(&atlas), 1);
        }
        assert_eq!(textures_created(&atlas), 5);
        Ok(())
    }

    #[test]
    fn idle_spare_is_released() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = image_key(1);

        insert_big(&atlas, key.clone());
        atlas.remove(&key);
        assert_eq!(spare_count(&atlas), 1);

        // Not idle long enough: kept.
        atlas
            .0
            .lock()
            .backend
            .release_idle_spares(Instant::now() + Duration::from_secs(59));
        assert_eq!(spare_count(&atlas), 1);

        atlas
            .0
            .lock()
            .backend
            .release_idle_spares(Instant::now() + Duration::from_secs(61));
        assert_eq!(spare_count(&atlas), 0);

        let created = textures_created(&atlas);
        insert_big(&atlas, key);
        assert_eq!(textures_created(&atlas), created + 1);
        Ok(())
    }

    #[test]
    fn clear_drops_spares() -> anyhow::Result<()> {
        let (device, queue) = test_device_and_queue()?;
        let atlas = WgpuAtlas::new(device, queue, wgpu::TextureFormat::Bgra8Unorm);
        let key = image_key(1);

        insert_big(&atlas, key.clone());
        atlas.remove(&key);
        assert_eq!(spare_count(&atlas), 1);
        atlas.clear();
        assert_eq!(spare_count(&atlas), 0);
        Ok(())
    }

    #[test]
    fn swizzle_upload_data_preserves_bgra_uploads() {
        let input = vec![0x10, 0x20, 0x30, 0x40];
        assert_eq!(
            swizzle_upload_data(&input, wgpu::TextureFormat::Bgra8Unorm),
            input
        );
    }

    #[test]
    fn swizzle_upload_data_converts_bgra_to_rgba() {
        let input = vec![0x10, 0x20, 0x30, 0x40, 0xAA, 0xBB, 0xCC, 0xDD];
        assert_eq!(
            swizzle_upload_data(&input, wgpu::TextureFormat::Rgba8Unorm),
            vec![0x30, 0x20, 0x10, 0x40, 0xCC, 0xBB, 0xAA, 0xDD]
        );
    }
}
