// SPDX-License-Identifier: MPL-2.0

//! The GPU preview kernel (feature `gpu`, ADR 0036): steps 3 to 7 of
//! `docs/MATHS.md` on one region of an image, on the GPU through wgpu, for
//! preview tiles only (step 8 preview). The kernel is `render.wgsl`, a
//! transcription of the fused CPU kernel of [`crate::render_region`], which
//! stays the reference and the only path for exports, thumbnails and scopes
//! (ground rule 3). Tests compare the two on every run where an adapter
//! exists: the positive within [`TOLERANCE`] per channel, and the display
//! encoding within one 8-bit step of the CPU tile.
//!
//! [`Gpu::render_region_display`] also encodes the positive for the screen
//! the way Little-CMS encodes 8-bit sRGB (the matrix from
//! `negg_cms::working_to_output_matrix`, the sRGB curve, 16-bit then 8-bit
//! rounding), so a tile comes back as RGBA8 with no CPU pass over it.

use std::fmt;
use std::sync::{Arc, mpsc};

use negg_types::{DensityModel, Estimates, Image};

use crate::full::{Region, check_estimates};
use crate::print::PrintCoefficients;

/// The largest difference per channel the GPU positive may have from the
/// CPU positive (the plan's 1/4096).
pub const TOLERANCE: f32 = 1.0 / 4096.0;

/// Threads per workgroup, as `@workgroup_size` in `render.wgsl`.
const WORKGROUP: u32 = 256;

/// Number of `vec4<f32>` parameter slots, as `p` in `render.wgsl`.
const SLOTS: usize = 19;

/// Why the GPU kernel could not run.
#[derive(Debug)]
pub enum GpuError {
    /// No GPU adapter or device (the caller renders on the CPU instead).
    Unavailable(String),
    /// The inputs failed the same checks as [`crate::render_region`].
    Invalid(negg_types::Error),
    /// The device failed while rendering or reading back.
    Failed(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::Unavailable(m) => write!(f, "no GPU: {m}"),
            GpuError::Invalid(e) => write!(f, "{e}"),
            GpuError::Failed(m) => write!(f, "the GPU render failed: {m}"),
        }
    }
}

impl std::error::Error for GpuError {}

impl From<negg_types::Error> for GpuError {
    fn from(e: negg_types::Error) -> Self {
        GpuError::Invalid(e)
    }
}

/// A GPU device with the two kernel pipelines of `render.wgsl`. `Send` and
/// `Sync`; calls from several threads queue on the device.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    positive: wgpu::ComputePipeline,
    display: wgpu::ComputePipeline,
    adapter: String,
}

impl fmt::Debug for Gpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gpu")
            .field("adapter", &self.adapter)
            .finish()
    }
}

/// The kernel's parameters, as the 19 four-word slots `render.wgsl`
/// documents: floats as their bits, and the pixel count and row width as
/// integers (sent as words, never as floats, so that no driver can flush
/// them as denormals).
fn params(
    est: &Estimates,
    print: &PrintCoefficients,
    density: Option<&DensityModel>,
    matrix: Option<[[f32; 3]; 3]>,
    display: [[f32; 3]; 3],
    pixels: u32,
    row: u32,
) -> [[u32; 4]; SLOTS] {
    let v = |a: [f32; 3], w: f32| [a[0], a[1], a[2], w];
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    // cast.rs correct_channel: an unusable slope is the identity, and
    // (e - 0) / 1 is e exactly.
    let usable = |slope: f32, offset: f32| {
        if slope != 0.0 && slope.is_finite() {
            (slope, offset)
        } else {
            (1.0, 0.0)
        }
    };
    let (sr, or) = usable(est.cast.slope_r, est.cast.offset_r);
    let (sb, ob) = usable(est.cast.slope_b, est.cast.offset_b);
    let model = density.copied().unwrap_or(DensityModel::IDENTITY);
    let m = matrix.unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    let mut p = [[0.0f32; 4]; SLOTS];
    p[0] = v(est.dmin, 0.0);
    p[1] = v(est.gamma, 0.0);
    p[2] = v(est.neutral_offset, 0.0);
    p[3] = [sr, or, sb, ob];
    p[4] = v(print.x0_total, print.k);
    p[5] = [print.dmin_p, print.dmax_p, print.a_hl, print.a_sh];
    p[6] = [
        print.floor,
        print.bpc_denominator,
        print.saturation,
        flag(print.bpc),
    ];
    p[7] = [
        flag(density.is_some()),
        flag(matrix.is_some()),
        // saturation.rs: exactly 1 skips the step.
        flag(print.saturation != 1.0),
        0.0,
    ];
    p[8] = v(model.a, 0.0);
    for c in 0..3 {
        p[9 + c] = v(model.g[c], 0.0);
        p[12 + c] = v(m[c], 0.0);
        p[15 + c] = v(display[c], 0.0);
    }
    let mut words = p.map(|slot| slot.map(f32::to_bits));
    words[7][3] = pixels;
    words[18][0] = row;
    words
}

impl Gpu {
    /// Opens the default high-performance adapter and builds the kernel.
    ///
    /// # Errors
    ///
    /// [`GpuError::Unavailable`] when there is no adapter or no device.
    pub fn new() -> Result<Gpu, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .map_err(|e| GpuError::Unavailable(e.to_string()))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("negg preview"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .map_err(|e| GpuError::Unavailable(e.to_string()))?;
        // Every call of `run` captures its own errors in error scopes; this
        // keeps anything else from panicking in wgpu's default handler.
        device.on_uncaptured_error(Arc::new(|_| {}));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("render.wgsl").into()),
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let positive = pipeline("positive");
        let display = pipeline("display");
        Ok(Gpu {
            device,
            queue,
            positive,
            display,
            adapter: format!("{} ({:?})", info.name, info.backend),
        })
    }

    /// The adapter's name and backend, for logs.
    #[must_use]
    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// Steps 3 to 7 of `docs/MATHS.md` on `region` of `image` on the GPU:
    /// [`crate::render_region`] within [`TOLERANCE`] per channel.
    ///
    /// # Errors
    ///
    /// [`GpuError::Invalid`] for the errors of [`crate::render_region`],
    /// and [`GpuError::Failed`] when the device fails.
    pub fn render_region(
        &self,
        image: &Image,
        region: Region,
        est: &Estimates,
        print: &PrintCoefficients,
        density: Option<&DensityModel>,
        matrix: Option<[[f32; 3]; 3]>,
    ) -> Result<Image, GpuError> {
        let input = self.prepare(image, region, est)?;
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let words = self.run(
            &self.positive,
            2,
            &input,
            |pixels, row| params(est, print, density, matrix, identity, pixels, row),
            3,
        )?;
        let data = bytemuck::cast_slice::<u32, f32>(&words).to_vec();
        Ok(Image::from_vec(input.width(), input.height(), data)?)
    }

    /// As [`Gpu::render_region`], then encoded for the screen through
    /// `display` (`negg_cms::working_to_output_matrix(OutputSpace::Srgb)`)
    /// and the sRGB curve, as RGBA8 (alpha 255), row-major: within one
    /// 8-bit step of the CPU tile encoded by Little-CMS.
    ///
    /// # Errors
    ///
    /// As [`Gpu::render_region`].
    #[allow(clippy::too_many_arguments)]
    pub fn render_region_display(
        &self,
        image: &Image,
        region: Region,
        est: &Estimates,
        print: &PrintCoefficients,
        density: Option<&DensityModel>,
        matrix: Option<[[f32; 3]; 3]>,
        display: [[f32; 3]; 3],
    ) -> Result<Vec<u8>, GpuError> {
        let input = self.prepare(image, region, est)?;
        let words = self.run(
            &self.display,
            3,
            &input,
            |pixels, row| params(est, print, density, matrix, display, pixels, row),
            1,
        )?;
        Ok(bytemuck::cast_slice::<u32, u8>(&words).to_vec())
    }

    /// The checks of [`crate::render_region`], then the region's pixels.
    fn prepare(&self, image: &Image, region: Region, est: &Estimates) -> Result<Image, GpuError> {
        check_estimates(est)?;
        Ok(image.crop(region.x, region.y, region.width, region.height)?)
    }

    /// Runs `pipeline` over every pixel of `input`, writing `words_per_pixel`
    /// 32-bit words per pixel to the output at `binding`, and reads them
    /// back.
    fn run(
        &self,
        pipeline: &wgpu::ComputePipeline,
        binding: u32,
        input: &Image,
        params: impl FnOnce(u32, u32) -> [[u32; 4]; SLOTS],
        words_per_pixel: u64,
    ) -> Result<Vec<u32>, GpuError> {
        use wgpu::util::DeviceExt;
        let pixels = input.width() * input.height();
        if pixels == 0 {
            return Ok(Vec::new());
        }
        let count = u32::try_from(pixels)
            .map_err(|_| GpuError::Failed(format!("{pixels} pixels is too many")))?;
        let groups = count.div_ceil(WORKGROUP);
        let max = self.device.limits().max_compute_workgroups_per_dimension;
        let (gx, gy) = if groups <= max {
            (groups, 1)
        } else {
            (max, groups.div_ceil(max))
        };
        let row = gx * WORKGROUP;
        let size = pixels as u64 * words_per_pixel * 4;
        let limits = self.device.limits();
        let largest = size.max(pixels as u64 * 12);
        if largest > limits.max_storage_buffer_binding_size || largest > limits.max_buffer_size {
            return Err(GpuError::Failed(format!(
                "{pixels} pixels need a {largest}-byte buffer, more than the device allows"
            )));
        }
        // Validation and out-of-memory errors are reported here, as
        // GpuError::Failed, instead of reaching wgpu's default handler
        // (which panics). Scopes are per thread and popped in reverse.
        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let src = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("negg preview input"),
                contents: bytemuck::cast_slice(input.data()),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let uniforms = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("negg preview parameters"),
                contents: bytemuck::cast_slice(&params(count, row)),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("negg preview output"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("negg preview readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: src.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        encoder.copy_buffer_to_buffer(&out, 0, &staging, 0, size);
        self.queue.submit([encoder.finish()]);
        let failed = pollster::block_on(validation.pop()).or(pollster::block_on(oom.pop()));
        if let Some(e) = failed {
            return Err(GpuError::Failed(e.to_string()));
        }
        let slice = staging.slice(..);
        let (tx, rx) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| GpuError::Failed(e.to_string()))?;
        rx.recv()
            .map_err(|e| GpuError::Failed(e.to_string()))?
            .map_err(|e| GpuError::Failed(e.to_string()))?;
        let view = slice
            .get_mapped_range()
            .map_err(|e| GpuError::Failed(e.to_string()))?;
        let words = bytemuck::cast_slice::<u8, u32>(&view).to_vec();
        drop(view);
        staging.unmap();
        Ok(words)
    }
}
