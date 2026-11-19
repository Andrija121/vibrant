use std::any::type_name;

use bytemuck::{bytes_of, checked::cast_slice};
use glam::{Mat4, UVec3, Vec3};
use half::f16;
use wgpu::{util::DeviceExt, wgt::TextureDataOrder, *};

use crate::{file::VolumeFile, gpu::Gpu};

#[derive(Debug, Clone)]
pub struct VolumeMaskSettings {
    pub name: String,
    pub visible: bool,
    pub sdf: bool,
    pub inverted: bool,
    pub offset: f32,
    pub width: f32,
}

pub struct VolumeMaskBuffer {
    texture: Texture,
    view: TextureView,
    // TODO(MASK-CPU-001): This CPU-side voxel copy is the prototype erase
    // storage. Replace it with a writable GPU 3D resection mask; retain only
    // GPU resources and immutable loaded-mask metadata in the final version.
    voxel_size: UVec3,
    transform: Mat4,
    data: Vec<f16>,
    // TODO(MASK-UNDO-001): Stroke snapshots will become GPU-resource
    // snapshots or small command records in the final undo/redo implementation.
    undo: Vec<Vec<f16>>,
    undo_requested: bool,
    settings: VolumeMaskSettings,
}

impl VolumeMaskBuffer {
    pub fn new(gpu: &Gpu, file: &VolumeFile) -> Self {
        let label = Some(type_name::<Self>());

        let size = Extent3d {
            width: file.size().x,
            height: file.size().y,
            depth_or_array_layers: file.size().z,
        };
        let data = file.data().to_vec();

        let texture = gpu.device().create_texture_with_data(
            gpu.queue(),
            &TextureDescriptor {
                label,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D3,
                format: TextureFormat::R16Float,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            },
            TextureDataOrder::LayerMajor,
            cast_slice(&data),
        );

        let settings = VolumeMaskSettings {
            name: file.name().to_string(),
            visible: true,
            offset: 0.0,
            width: 0.02,
            inverted: false,
            sdf: file.is_signed(),
        };

        Self::from_texture_settings(
            texture,
            settings,
            file.size(),
            file.transform(),
            data,
        )
    }

    pub fn size(&self) -> Extent3d {
        self.texture.size()
    }

    pub fn view(&self) -> &TextureView {
        &self.view
    }

    pub fn settings_mut(&mut self) -> &mut VolumeMaskSettings {
        &mut self.settings
    }

    pub fn settings(&self) -> &VolumeMaskSettings {
        &self.settings
    }

    pub fn voxel_size(&self) -> UVec3 {
        self.voxel_size
    }

    pub fn transform(&self) -> Mat4 {
        self.transform
    }

    pub fn request_undo(&mut self) {
        self.undo_requested = true;
    }

    pub fn undo(&mut self, gpu: &Gpu) {
        if !self.undo_requested {
            return;
        }
        self.undo_requested = false;

        if let Some(previous) = self.undo.pop() {
            self.data = previous;
            self.upload(gpu);
        }
    }

    /// Finds the first mask voxel intersected by a world-space ray.
    // TODO(MASK-PICK-001): This CPU AABB/ray traversal is only for the
    // prototype. A GPU compute shader can edit a sphere directly once the
    // brush center and radius are uploaded to GPU.
    pub fn ray_hit(&self, origin: Vec3, direction: Vec3) -> Option<Vec3> {
        let direction = direction.normalize();
        let inverse = self.transform.inverse();
        let corners = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
        ];
        let world_corners = corners.map(|corner| inverse.transform_point3(corner));
        let box_min = world_corners
            .iter()
            .copied()
            .fold(Vec3::splat(f32::INFINITY), Vec3::min);
        let box_max = world_corners
            .iter()
            .copied()
            .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        let inv_direction = Vec3::select(
            direction.abs().cmplt(Vec3::splat(1e-6)),
            Vec3::splat(1e20),
            Vec3::ONE / direction,
        );
        let t1 = (box_min - origin) * inv_direction;
        let t2 = (box_max - origin) * inv_direction;
        let t_min = t1.min(t2).max_element();
        let t_max = t1.max(t2).min_element();
        if t_min > t_max || t_max < 0.0 {
            return None;
        }

        let start = t_min.max(0.0);
        let end = t_max;
        let steps = 4096u32;
        for step in 0..=steps {
            let distance = start + (end - start) * step as f32 / steps as f32;
            let world = origin + direction * distance;
            let uv = self.transform.transform_point3(world);
            if uv.x < 0.0
                || uv.y < 0.0
                || uv.z < 0.0
                || uv.x > 1.0
                || uv.y > 1.0
                || uv.z > 1.0
            {
                continue;
            }

            let voxel = (uv * self.voxel_size.as_vec3()).as_uvec3();
            let voxel = UVec3::new(
                voxel.x.min(self.voxel_size.x.saturating_sub(1)),
                voxel.y.min(self.voxel_size.y.saturating_sub(1)),
                voxel.z.min(self.voxel_size.z.saturating_sub(1)),
            );
            let index = voxel.x as usize
                + voxel.y as usize * self.voxel_size.x as usize
                + voxel.z as usize * self.voxel_size.x as usize * self.voxel_size.y as usize;
            if let Some(value) = self.data.get(index) {
                if value.to_f32() > 0.5 {
                    let uv_voxel = (voxel.as_vec3() + Vec3::splat(0.5))
                        / self.voxel_size.as_vec3();
                    return Some(inverse.transform_point3(uv_voxel));
                }
            }
        }
        None
    }

    /// Erases a spherical region from the in-memory mask and uploads it to GPU.
    // TODO(MASK-CPU-002): The nested CPU voxel loop and write_texture upload
    // are temporary. Replace with a WGSL compute shader that writes 0 for
    // voxels inside the brush sphere in the dedicated resection texture.
    pub fn erase_sphere(&mut self, gpu: &Gpu, center: Vec3, radius: f32, stroke_start: bool) {
        if stroke_start {
            self.undo.push(self.data.clone());
        }

        let inverse = self.transform.inverse();
        let radius_squared = radius * radius;
        let mut changed = false;

        for z in 0..self.voxel_size.z {
            for y in 0..self.voxel_size.y {
                for x in 0..self.voxel_size.x {
                    let uv = (UVec3::new(x, y, z).as_vec3() + Vec3::splat(0.5))
                        / self.voxel_size.as_vec3();
                    let world = inverse.transform_point3(uv);
                    if world.distance_squared(center) <= radius_squared {
                        let index = x as usize
                            + y as usize * self.voxel_size.x as usize
                            + z as usize * self.voxel_size.x as usize * self.voxel_size.y as usize;
                        if self.data[index].to_f32() != 0.0 {
                            self.data[index] = f16::from_f32(0.0);
                            changed = true;
                        }
                    }
                }
            }
        }

        if changed {
            self.upload(gpu);
        } else if stroke_start {
            self.undo.pop();
        }
    }

    fn upload(&self, gpu: &Gpu) {
        let row_bytes = self.voxel_size.x as usize * std::mem::size_of::<f16>();
        let bytes_per_row = (row_bytes + 255) & !255;
        let mut padded = vec![0u8; bytes_per_row * self.voxel_size.y as usize * self.voxel_size.z as usize];

        for z in 0..self.voxel_size.z as usize {
            for y in 0..self.voxel_size.y as usize {
                let source_start = (z * self.voxel_size.y as usize + y)
                    * self.voxel_size.x as usize;
                let dest_start = (z * self.voxel_size.y as usize + y) * bytes_per_row;
                let source_bytes = cast_slice::<f16, u8>(&self.data[source_start..source_start + self.voxel_size.x as usize]);
                padded[dest_start..dest_start + source_bytes.len()].copy_from_slice(source_bytes);
            }
        }

        gpu.queue().write_texture(
            TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &padded,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row as u32),
                rows_per_image: Some(self.voxel_size.y),
            },
            Extent3d {
                width: self.voxel_size.x,
                height: self.voxel_size.y,
                depth_or_array_layers: self.voxel_size.z,
            },
        );
    }

    pub fn none(gpu: &Gpu) -> VolumeMaskBuffer {
        let label = Some(type_name::<Self>());

        let texture = gpu.device().create_texture_with_data(
            gpu.queue(),
            &TextureDescriptor {
                label,
                size: Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D3,
                format: TextureFormat::R16Float,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            },
            TextureDataOrder::LayerMajor,
            bytes_of(&1.0),
        );

        let settings = VolumeMaskSettings {
            name: "None".to_string(),
            visible: false,
            offset: 0.0,
            width: 0.00,
            inverted: false,
            sdf: false,
        };

        let size = UVec3::new(1, 1, 1);
        let data = vec![f16::from_f32(1.0)];
        Self::from_texture_settings(texture, settings, size, Mat4::IDENTITY, data)
    }

    fn from_texture_settings(
        texture: Texture,
        settings: VolumeMaskSettings,
        size: UVec3,
        transform: Mat4,
        data: Vec<f16>,
    ) -> Self {
        let view = texture.create_view(&TextureViewDescriptor::default());

        Self {
            texture,
            view,
            voxel_size: size,
            transform,
            data,
            undo: Vec::new(),
            undo_requested: false,
            settings,
        }
    }
}

impl Drop for VolumeMaskBuffer {
    fn drop(&mut self) {
        self.texture.destroy();
    }
}
