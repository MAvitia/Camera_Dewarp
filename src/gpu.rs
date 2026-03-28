use crate::remap::RemapLut;
use wgpu::util::DeviceExt;

pub struct GpuRemapper {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    lut_x_buf: Option<wgpu::Buffer>,
    lut_y_buf: Option<wgpu::Buffer>,
    cached_dims: (u32, u32),
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    src_width: u32,
    src_height: u32,
}

impl GpuRemapper {
    pub fn new() -> Option<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::DX12,
            ..Default::default()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()?;

        log::info!("GPU adapter: {}", adapter.get_info().name);

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("dewarp-gpu"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits {
                    max_storage_buffer_binding_size: 512 * 1024 * 1024,
                    max_buffer_size: 512 * 1024 * 1024,
                    ..Default::default()
                },
                ..Default::default()
            },
        ))
        .ok()?;

        let shader_src = include_str!("../shaders/remap.wgsl");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("remap-shader"),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("remap-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("remap-pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("remap-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Some(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            lut_x_buf: None,
            lut_y_buf: None,
            cached_dims: (0, 0),
        })
    }

    pub fn upload_lut(&mut self, lut: &RemapLut) {
        if self.cached_dims == (lut.width, lut.height) {
            return;
        }

        self.lut_x_buf = Some(
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("lut-x"),
                    contents: bytemuck::cast_slice(&lut.map_x),
                    usage: wgpu::BufferUsages::STORAGE,
                }),
        );

        self.lut_y_buf = Some(
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("lut-y"),
                    contents: bytemuck::cast_slice(&lut.map_y),
                    usage: wgpu::BufferUsages::STORAGE,
                }),
        );

        self.cached_dims = (lut.width, lut.height);
    }

    pub fn remap(
        &self,
        src_rgb: &[u8],
        src_width: u32,
        src_height: u32,
        dst_width: u32,
        dst_height: u32,
    ) -> Vec<u8> {
        let lut_x_buf = self.lut_x_buf.as_ref().expect("LUT not uploaded");
        let lut_y_buf = self.lut_y_buf.as_ref().expect("LUT not uploaded");

        let pixel_count_src = (src_width * src_height) as usize;
        let mut src_rgba = vec![0u32; pixel_count_src];
        for i in 0..pixel_count_src {
            let r = src_rgb[i * 3] as u32;
            let g = src_rgb[i * 3 + 1] as u32;
            let b = src_rgb[i * 3 + 2] as u32;
            src_rgba[i] = r | (g << 8) | (b << 16) | (255 << 24);
        }

        let pixel_count_dst = (dst_width * dst_height) as usize;

        let params = Params {
            width: dst_width,
            height: dst_height,
            src_width,
            src_height,
        };

        let params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let src_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("src"),
                contents: bytemuck::cast_slice(&src_rgba),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let dst_size = (pixel_count_dst * 4) as u64;
        let dst_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dst"),
            size: dst_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let readback_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: dst_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("remap-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: src_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: lut_x_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: lut_y_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: dst_buf.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("remap-enc"),
            });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("remap-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let wg_x = (dst_width + 15) / 16;
            let wg_y = (dst_height + 15) / 16;
            pass.dispatch_workgroups(wg_x, wg_y, 1);
        }

        encoder.copy_buffer_to_buffer(&dst_buf, 0, &readback_buf, 0, dst_size);
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = readback_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).ok();
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();

        let data = slice.get_mapped_range();
        let dst_rgba: &[u32] = bytemuck::cast_slice(&data);

        let mut out = vec![0u8; pixel_count_dst * 3];
        for i in 0..pixel_count_dst {
            let packed = dst_rgba[i];
            out[i * 3] = (packed & 0xFF) as u8;
            out[i * 3 + 1] = ((packed >> 8) & 0xFF) as u8;
            out[i * 3 + 2] = ((packed >> 16) & 0xFF) as u8;
        }

        drop(data);
        readback_buf.unmap();

        out
    }
}
