//! The observer page's unit silhouettes, as `CSceneRenderer::
//! RenderSilhouettes` draws them: the occluders (the ground, structures and
//! gaia) into a depth buffer of their own, then the units where that depth
//! is nearer, flat in their player colour. Drawn onto the screen after the
//! post-processing, as the engine's `RenderSceneOverlays`.

/// Its own buffer, as the engine clears the depth for the pass: the
/// scene's depth also holds the units, which would show one unit behind
/// another.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
/// `effects/model.xml` MODE_SILHOUETTEDISPLAY's blend constant, 191/255.
const OPACITY: f64 = 191.0 / 255.0;

pub struct Silhouette {
    pub terrain_pipeline: wgpu::RenderPipeline,
    pub occluder_pipeline: wgpu::RenderPipeline,
    pub caster_pipeline: wgpu::RenderPipeline,
    target: Option<(wgpu::TextureView, (u32, u32))>,
}

impl Silhouette {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        view_layout: &wgpu::BindGroupLayout,
        model_layout: &wgpu::PipelineLayout,
        model_shader: &wgpu::ShaderModule,
        model_buffers: &[Option<wgpu::VertexBufferLayout>],
        field_shader: &wgpu::ShaderModule,
        terrain_buffers: &[Option<wgpu::VertexBufferLayout>],
    ) -> Self {
        // The pass draws onto the screen, so every pipeline names its
        // format; the occluders write depth only.
        let depth_only = [Some(wgpu::ColorTargetState {
            format,
            blend: None,
            write_mask: wgpu::ColorWrites::empty(),
        })];
        let empty_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("silhouette terrain"),
            source: wgpu::ShaderSource::Wgsl("@fragment fn fs_none() {}".into()),
        });
        let occluder_depth = wgpu::DepthStencilState {
            format: FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        };
        let terrain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("silhouette terrain"),
            bind_group_layouts: &[Some(view_layout)],
            immediate_size: 0,
        });
        let terrain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("silhouette terrain"),
            layout: Some(&terrain_layout),
            vertex: wgpu::VertexState {
                module: field_shader,
                entry_point: Some("vs_terrain"),
                compilation_options: Default::default(),
                buffers: terrain_buffers,
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(occluder_depth.clone()),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &empty_shader,
                entry_point: Some("fs_none"),
                compilation_options: Default::default(),
                targets: &depth_only,
            }),
            multiview_mask: None,
            cache: None,
        });
        let occluder_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("silhouette occluders"),
            layout: Some(model_layout),
            vertex: wgpu::VertexState {
                module: model_shader,
                entry_point: Some("vs_model"),
                compilation_options: Default::default(),
                buffers: model_buffers,
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(occluder_depth),
            multisample: Default::default(),
            // The shadow pass's alpha test, so a unit shows through the
            // gaps between a tree's leaves.
            fragment: Some(wgpu::FragmentState {
                module: model_shader,
                entry_point: Some("fs_shadow"),
                compilation_options: Default::default(),
                targets: &depth_only,
            }),
            multiview_mask: None,
            cache: None,
        });
        // Each pixel once, in whichever unit's colour gets there first, as
        // the engine cannot sort them either.
        let once = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::NotEqual,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::Replace,
        };
        let caster_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("silhouette casters"),
            layout: Some(model_layout),
            vertex: wgpu::VertexState {
                module: model_shader,
                entry_point: Some("vs_model"),
                compilation_options: Default::default(),
                buffers: model_buffers,
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: wgpu::StencilState {
                    front: once,
                    back: once,
                    read_mask: !0,
                    write_mask: !0,
                },
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: model_shader,
                entry_point: Some("fs_silhouette"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Constant,
                            dst_factor: wgpu::BlendFactor::OneMinusConstant,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Silhouette {
            terrain_pipeline,
            occluder_pipeline,
            caster_pipeline,
            target: None,
        }
    }

    /// Frees the depth buffer while silhouettes are off.
    pub fn release(&mut self) {
        self.target = None;
    }

    /// Starts the pass over `screen`, of `size`, its depth and stencil
    /// cleared.
    pub fn begin<'e>(
        &mut self,
        device: &wgpu::Device,
        encoder: &'e mut wgpu::CommandEncoder,
        screen: &wgpu::TextureView,
        size: (u32, u32),
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites>,
    ) -> wgpu::RenderPass<'e> {
        if self.target.as_ref().is_none_or(|(_, made)| *made != size) {
            self.target = Some((create_target(device, size), size));
        }
        let (target, _) = self.target.as_ref().expect("made above");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("silhouettes"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: screen,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0),
                    store: wgpu::StoreOp::Discard,
                }),
            }),
            timestamp_writes,
            ..Default::default()
        });
        pass.set_stencil_reference(1);
        pass.set_blend_constant(wgpu::Color {
            r: OPACITY,
            g: OPACITY,
            b: OPACITY,
            a: OPACITY,
        });
        pass
    }
}

fn create_target(device: &wgpu::Device, (width, height): (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("silhouette depth"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
