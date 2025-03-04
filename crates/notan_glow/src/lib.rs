use glow::*;
use hashbrown::HashMap;
use notan_graphics::prelude::*;
use notan_graphics::DeviceBackend;

mod buffer;
mod pipeline;
mod render_target;
mod texture;
mod to_glow;
mod utils;

use crate::buffer::Kind;
use crate::pipeline::get_inner_attrs;
use crate::texture::texture_format;
use crate::to_glow::ToGlow;
use buffer::InnerBuffer;
use pipeline::{InnerPipeline, VertexAttributes};
use render_target::InnerRenderTexture;
use texture::InnerTexture;

pub struct GlowBackend {
    gl: Context,
    buffer_count: u64,
    texture_count: u64,
    pipeline_count: u64,
    render_target_count: u64,
    size: (i32, i32),
    dpi: f32,
    pipelines: HashMap<u64, InnerPipeline>,
    buffers: HashMap<u64, InnerBuffer>,
    textures: HashMap<u64, InnerTexture>,
    render_targets: HashMap<u64, InnerRenderTexture>,
    using_indices: bool,
    api_name: String,
    current_pipeline: u64,
    limits: Limits,
    current_uniforms: Vec<UniformLocation>,
    drawing_srgba: bool,
    drawing_to_render_texture: bool,
}

impl GlowBackend {
    #[cfg(target_arch = "wasm32")]
    pub fn new(
        canvas: &web_sys::HtmlCanvasElement,
        antialias: bool,
        transparent: bool,
    ) -> Result<Self, String> {
        let (gl, api) = utils::create_gl_context(canvas, antialias, transparent)?;
        Self::from(gl, &api)
    }

    #[cfg(all(
        not(target_arch = "wasm32"),
        not(target_os = "ios"),
        not(target_os = "android")
    ))]
    pub fn new<F>(loader_function: F) -> Result<Self, String>
    where
        F: FnMut(&str) -> *const std::os::raw::c_void,
    {
        let gl = unsafe { Context::from_loader_function(loader_function) };

        Self::from(gl, "opengl")
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub fn new<F>(mut loader_function: F) -> Result<Self, String>
    where
        F: FnMut(&str) -> *const std::os::raw::c_void,
    {
        let gl = unsafe { Context::from_loader_function(loader_function) };

        Self::from(gl, "opengl_es")
    }

    fn from(gl: Context, api: &str) -> Result<Self, String> {
        log::info!("Using {} graphics api", api);

        let limits = unsafe {
            Limits {
                max_texture_size: gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as _,
                max_uniform_blocks: gl.get_parameter_i32(glow::MAX_UNIFORM_BLOCK_SIZE) as _,
            }
        };

        Ok(Self {
            pipeline_count: 0,
            buffer_count: 0,
            texture_count: 0,
            render_target_count: 0,
            gl,
            size: (0, 0),
            dpi: 1.0,
            pipelines: HashMap::new(),
            buffers: HashMap::new(),
            textures: HashMap::new(),
            render_targets: HashMap::new(),
            using_indices: false,
            api_name: api.to_string(),
            current_pipeline: 0,
            limits,
            current_uniforms: vec![],
            drawing_srgba: false,
            drawing_to_render_texture: false,
        })
    }
}

impl GlowBackend {
    #[inline(always)]
    fn clear(&self, color: &Option<Color>, depth: &Option<f32>, stencil: &Option<i32>) {
        clear(&self.gl, color, depth, stencil);
    }

    #[inline]
    fn enable_srgba(&mut self) {
        if self.drawing_srgba {
            return;
        }

        self.drawing_srgba = true;
        unsafe {
            self.gl.enable(glow::FRAMEBUFFER_SRGB);
        }
    }

    #[inline]
    fn disable_srgba(&mut self) {
        if !self.drawing_srgba {
            return;
        }

        self.drawing_srgba = false;
        unsafe {
            self.gl.disable(glow::FRAMEBUFFER_SRGB);
        }
    }

    fn begin(
        &mut self,
        target: Option<u64>,
        color: &Option<Color>,
        depth: &Option<f32>,
        stencil: &Option<i32>,
    ) {
        let render_target = match target {
            Some(id) => self.render_targets.get(&id),
            _ => None,
        };

        let (width, height, dpi) = match render_target {
            Some(rt) => {
                rt.bind(&self.gl);
                self.drawing_to_render_texture = true;
                (rt.size.0, rt.size.1, 1.0)
            }
            None => {
                unsafe {
                    self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                }
                self.drawing_to_render_texture = false;
                (self.size.0, self.size.1, self.dpi)
            }
        };

        self.viewport(0.0, 0.0, width as _, height as _, dpi);

        self.clear(color, depth, stencil);
    }

    #[inline]
    fn viewport(&self, mut x: f32, mut y: f32, width: f32, height: f32, dpi: f32) {
        if !self.drawing_to_render_texture {
            y = (self.size.1 as f32 - (height + y)) * dpi;
            x *= dpi;
        }
        let ww = width * dpi;
        let hh = height * dpi;

        unsafe {
            self.gl.viewport(x as _, y as _, ww as _, hh as _);
        }
    }

    #[inline]
    fn scissors(&self, x: f32, y: f32, width: f32, height: f32, dpi: f32) {
        let canvas_height = ((self.size.1 - (height + y) as i32) as f32 * dpi) as i32;
        let x = x * dpi;
        let width = width * dpi;
        let height = height * dpi;

        unsafe {
            self.gl.enable(glow::SCISSOR_TEST);
            self.gl
                .scissor(x as _, canvas_height, width as _, height as _);
        }
    }

    fn end(&mut self) {
        unsafe {
            self.disable_srgba();
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.bind_buffer(glow::ARRAY_BUFFER, None);
            self.gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            self.gl.bind_buffer(glow::UNIFORM_BUFFER, None);
            self.gl.bind_vertex_array(None);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }

        self.using_indices = false;
        self.drawing_to_render_texture = false;
    }

    fn clean_pipeline(&mut self, id: u64) {
        if let Some(pip) = self.pipelines.remove(&id) {
            pip.clean(&self.gl);
        }
    }

    fn set_pipeline(&mut self, id: u64, options: &PipelineOptions) {
        if let Some(pip) = self.pipelines.get(&id) {
            pip.bind(&self.gl, options);
            self.using_indices = false;
            self.current_pipeline = id;
            self.current_uniforms = pip.uniform_locations.clone();
        }
    }

    fn bind_buffer(&mut self, id: u64) {
        if let Some(buffer) = self.buffers.get_mut(&id) {
            #[cfg(debug_assertions)]
            {
                debug_assert!(
                    buffer.initialized,
                    "Buffer {} -> id({}) is doesn't contain data. This can cause Undefined behavior.",
                    buffer.kind,
                    id
                )
            }
            let reset_attrs = match &buffer.kind {
                Kind::Index => {
                    self.using_indices = true;
                    false
                }
                Kind::Uniform(_slot, _name) => {
                    if !buffer.block_binded {
                        buffer.bind_ubo_block(
                            &self.gl,
                            self.pipelines.get(&self.current_pipeline).as_ref().unwrap(),
                        );
                    }
                    false
                }
                Kind::Vertex(attrs) => match self.pipelines.get_mut(&self.current_pipeline) {
                    Some(pip) => pip.use_attrs(id, attrs),
                    _ => false,
                },
            };

            buffer.bind(&self.gl, Some(self.current_pipeline), reset_attrs);
        }
    }

    fn bind_texture(&mut self, id: u64, slot: u32, location: u32) {
        if let Some(pip) = self.pipelines.get(&self.current_pipeline) {
            let is_srgba = if let Some(texture) = self.textures.get(&id) {
                #[cfg(debug_assertions)]
                if !pip.texture_locations.contains_key(&location) {
                    log::warn!("Uniform location {} for texture {} should be declared when the pipeline is created.", location, id);
                }

                let loc = pip
                    .texture_locations
                    .get(&location)
                    .unwrap_or_else(|| self.get_texture_uniform_loc(&location));
                texture.bind(&self.gl, slot, loc);
                texture.is_srgba
            } else {
                false
            };

            if is_srgba {
                self.enable_srgba();
            } else {
                self.disable_srgba();
            }
        }
    }

    #[inline(always)]
    fn get_texture_uniform_loc<'a>(&'a self, location: &'a u32) -> &'a UniformLocation {
        if cfg!(debug_assertions) {
            self.current_uniforms.get(*location as usize)
                .as_ref()
                .ok_or_else(|| format!("Invalid uniform location {}, this could means that you're trying to access a unifor not used in the shader code.", location))
                .unwrap()
        } else {
            &self.current_uniforms[*location as usize]
        }
    }

    fn clean_buffer(&mut self, id: u64) {
        if let Some(buffer) = self.buffers.remove(&id) {
            buffer.clean(&self.gl);
        }
    }

    fn clean_texture(&mut self, id: u64) {
        if let Some(texture) = self.textures.remove(&id) {
            texture.clean(&self.gl);
        }
    }

    fn clean_render_target(&mut self, id: u64) {
        if let Some(rt) = self.render_targets.remove(&id) {
            rt.clean(&self.gl);
        }
    }

    fn draw(&mut self, primitive: &DrawPrimitive, offset: i32, count: i32) {
        unsafe {
            if self.using_indices {
                self.gl
                    .draw_elements(primitive.to_glow(), count, glow::UNSIGNED_INT, offset * 4);
            } else {
                self.gl.draw_arrays(primitive.to_glow(), offset, count);
            }
        }
    }
    fn draw_instanced(&mut self, primitive: &DrawPrimitive, offset: i32, count: i32, length: i32) {
        unsafe {
            if self.using_indices {
                self.gl.draw_elements_instanced(
                    primitive.to_glow(),
                    count,
                    glow::UNSIGNED_INT,
                    offset,
                    length,
                );
            } else {
                self.gl
                    .draw_arrays_instanced(primitive.to_glow(), offset, count, length);
            }
        }
    }
}

impl DeviceBackend for GlowBackend {
    fn api_name(&self) -> &str {
        &self.api_name
    }

    fn limits(&self) -> Limits {
        self.limits
    }

    fn create_pipeline(
        &mut self,
        vertex_source: &[u8],
        fragment_source: &[u8],
        vertex_attrs: &[VertexAttr],
        texture_locations: &[(u32, String)],
        options: PipelineOptions,
    ) -> Result<u64, String> {
        let vertex_source = std::str::from_utf8(vertex_source).map_err(|e| e.to_string())?;
        let fragment_source = std::str::from_utf8(fragment_source).map_err(|e| e.to_string())?;

        let inner_pipeline = InnerPipeline::new(
            &self.gl,
            vertex_source,
            fragment_source,
            vertex_attrs,
            texture_locations,
        )?;
        inner_pipeline.bind(&self.gl, &options);

        self.pipeline_count += 1;
        self.pipelines.insert(self.pipeline_count, inner_pipeline);

        self.set_pipeline(self.pipeline_count, &options);
        Ok(self.pipeline_count)
    }

    fn create_vertex_buffer(
        &mut self,
        attrs: &[VertexAttr],
        step_mode: VertexStepMode,
    ) -> Result<u64, String> {
        let (stride, inner_attrs) = get_inner_attrs(attrs);
        let kind = Kind::Vertex(VertexAttributes::new(stride, inner_attrs, step_mode));
        let mut inner_buffer = InnerBuffer::new(&self.gl, kind, true)?;
        inner_buffer.bind(&self.gl, Some(self.current_pipeline), false);
        self.buffer_count += 1;
        self.buffers.insert(self.buffer_count, inner_buffer);
        Ok(self.buffer_count)
    }

    fn create_index_buffer(&mut self) -> Result<u64, String> {
        let mut inner_buffer = InnerBuffer::new(&self.gl, Kind::Index, true)?;
        inner_buffer.bind(&self.gl, Some(self.current_pipeline), false);
        self.buffer_count += 1;
        self.buffers.insert(self.buffer_count, inner_buffer);
        Ok(self.buffer_count)
    }

    fn create_uniform_buffer(&mut self, slot: u32, name: &str) -> Result<u64, String> {
        let mut inner_buffer =
            InnerBuffer::new(&self.gl, Kind::Uniform(slot, name.to_string()), true)?;
        inner_buffer.bind(&self.gl, Some(self.current_pipeline), false);
        self.buffer_count += 1;
        self.buffers.insert(self.buffer_count, inner_buffer);
        Ok(self.buffer_count)
    }

    fn set_buffer_data(&mut self, id: u64, data: &[u8]) {
        if let Some(buffer) = self.buffers.get_mut(&id) {
            buffer.bind(&self.gl, None, false);
            buffer.update(&self.gl, data);
        }
    }

    fn render(&mut self, commands: &[Commands], target: Option<u64>) {
        commands.iter().for_each(|cmd| {
            use Commands::*;
            // println!("Render cmd: {:?}", cmd);

            match cmd {
                Begin {
                    color,
                    depth,
                    stencil,
                } => self.begin(target, color, depth, stencil),
                End => self.end(),
                Pipeline { id, options } => self.set_pipeline(*id, options),
                BindBuffer { id } => self.bind_buffer(*id),
                Draw {
                    primitive,
                    offset,
                    count,
                } => self.draw(primitive, *offset, *count),
                DrawInstanced {
                    primitive,
                    offset,
                    count,
                    length,
                } => self.draw_instanced(primitive, *offset, *count, *length),
                BindTexture { id, slot, location } => self.bind_texture(*id, *slot, *location),
                Size { width, height } => self.set_size(*width, *height),
                Viewport {
                    x,
                    y,
                    width,
                    height,
                } => self.viewport(*x, *y, *width, *height, self.dpi),
                Scissors {
                    x,
                    y,
                    width,
                    height,
                } => self.scissors(*x, *y, *width, *height, self.dpi),
            }
        });
    }

    fn clean(&mut self, to_clean: &[ResourceId]) {
        log::debug!("gpu resources to_clean {:?}", to_clean);
        to_clean.iter().for_each(|res| match &res {
            ResourceId::Pipeline(id) => self.clean_pipeline(*id),
            ResourceId::Buffer(id) => self.clean_buffer(*id),
            ResourceId::Texture(id) => self.clean_texture(*id),
            ResourceId::RenderTexture(id) => self.clean_render_target(*id),
        })
    }

    fn set_size(&mut self, width: i32, height: i32) {
        self.size = (width, height);
    }

    fn set_dpi(&mut self, scale_factor: f64) {
        self.dpi = scale_factor as _;
    }

    fn create_texture(&mut self, info: &TextureInfo) -> Result<u64, String> {
        let inner_texture = InnerTexture::new(&self.gl, info)?;
        self.texture_count += 1;
        self.textures.insert(self.texture_count, inner_texture);
        Ok(self.texture_count)
    }

    fn create_render_texture(
        &mut self,
        texture_id: u64,
        info: &TextureInfo,
    ) -> Result<u64, String> {
        let texture = self.textures.get(&texture_id).ok_or(format!(
            "Error creating render target: texture id '{}' not found.",
            texture_id
        ))?;

        let inner_rt = InnerRenderTexture::new(&self.gl, texture, info)?;
        self.render_target_count += 1;
        self.render_targets
            .insert(self.render_target_count, inner_rt);
        Ok(self.render_target_count)
    }

    fn update_texture(&mut self, texture: u64, opts: &TextureUpdate) -> Result<(), String> {
        match self.textures.get(&texture) {
            Some(texture) => {
                unsafe {
                    self.gl
                        .bind_texture(glow::TEXTURE_2D, Some(texture.texture));
                    self.gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        opts.x_offset,
                        opts.y_offset,
                        opts.width,
                        opts.height,
                        texture_format(&opts.format), // 3d texture needs another value?
                        glow::UNSIGNED_BYTE,          // todo UNSIGNED SHORT FOR DEPTH (3d) TEXTURES
                        PixelUnpackData::Slice(opts.bytes),
                    );
                    // todo unbind texture?
                    Ok(())
                }
            }
            _ => Err("Invalid texture id".to_string()),
        }
    }

    fn read_pixels(
        &mut self,
        texture: u64,
        bytes: &mut [u8],
        opts: &TextureRead,
    ) -> Result<(), String> {
        match self.textures.get(&texture) {
            Some(texture) => unsafe {
                let fbo = self.gl.create_framebuffer()?;
                self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                self.gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture.texture),
                    0,
                );

                let status = self.gl.check_framebuffer_status(glow::FRAMEBUFFER);
                let can_read = status == glow::FRAMEBUFFER_COMPLETE;

                let clean = || {
                    self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    self.gl.delete_framebuffer(fbo);
                };

                if can_read {
                    self.gl.read_pixels(
                        opts.x_offset,
                        opts.y_offset,
                        opts.width,
                        opts.height,
                        texture_format(&opts.format),
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(bytes),
                    );
                    clean();
                    Ok(())
                } else {
                    clean();
                    Err("Framebuffer incomplete...".to_string())
                }
            },
            None => Err("Invalid texture id".to_string()),
        }
    }
}

#[inline]
pub(crate) fn clear(
    gl: &Context,
    color: &Option<Color>,
    depth: &Option<f32>,
    stencil: &Option<i32>,
) {
    let mut mask = 0;
    unsafe {
        if let Some(color) = color {
            mask |= glow::COLOR_BUFFER_BIT;
            gl.clear_color(color.r, color.g, color.b, color.a);
        }

        if let Some(depth) = *depth {
            mask |= glow::DEPTH_BUFFER_BIT;
            gl.enable(glow::DEPTH_TEST);
            gl.depth_mask(true);
            gl.clear_depth_f32(depth);
        }

        if let Some(stencil) = *stencil {
            mask |= glow::STENCIL_BUFFER_BIT;
            gl.enable(glow::STENCIL_TEST);
            gl.stencil_mask(0xff);
            gl.clear_stencil(stencil);
        }

        if mask != 0 {
            gl.clear(mask);
        }
    }
}
