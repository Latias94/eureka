use crate::vulkan::adapter::Adapter;
use crate::vulkan::command_buffer::{CommandBuffer, CommandBufferState};
use crate::vulkan::command_buffer_allocator::CommandBufferAllocator;
use crate::vulkan::conv;
use crate::vulkan::device::Device;
use crate::vulkan::instance::Instance;
use crate::vulkan::pipeline::Pipeline;
use crate::vulkan::render_pass::{RenderPass, RenderPassDescriptor};
use crate::vulkan::shader::{Shader, ShaderDescriptor};
use crate::vulkan::surface::Surface;
use crate::vulkan::texture::{Texture, TextureDescriptor};
use crate::vulkan::texture_view::TextureView;
use crate::{Color, DeviceError, QueueFamilyIndices, SurfaceError};
use ash::extensions::khr;
use ash::vk;
use gpu_allocator::vulkan::Allocator;
use parking_lot::Mutex;
use std::rc::Rc;
use typed_builder::TypedBuilder;

pub struct Swapchain {
    raw: vk::SwapchainKHR,
    loader: khr::Swapchain,
    device: Rc<Device>,
    family_index: QueueFamilyIndices,
    textures: Vec<vk::Image>,
    texture_views: Vec<TextureView>,
    surface_format: vk::SurfaceFormatKHR,
    depth_format: vk::Format,
    extent: vk::Extent2D,
    capabilities: vk::SurfaceCapabilitiesKHR,
    render_pass: RenderPass,
    pipeline: Pipeline,
    command_buffers: Vec<CommandBuffer>,
    framebuffers: Vec<vk::Framebuffer>,
    graphics_queue: vk::Queue,
    present_queue: vk::Queue,
    command_buffer_allocator: CommandBufferAllocator,
    depth_texture: Texture,
    depth_texture_view: TextureView,
}

#[derive(Clone, Copy, Debug)]
struct SwapchainProperties {
    pub surface_format: vk::SurfaceFormatKHR,
    pub present_mode: vk::PresentModeKHR,
    pub extent: vk::Extent2D,
}

struct SwapChainSupportDetail {
    pub capabilities: vk::SurfaceCapabilitiesKHR,
    pub surface_formats: Vec<vk::SurfaceFormatKHR>,
    pub present_modes: Vec<vk::PresentModeKHR>,
}

#[derive(Clone, TypedBuilder)]
pub struct SwapchainDescriptor<'a> {
    pub adapter: &'a Adapter,
    pub surface: &'a Surface,
    pub instance: &'a Instance,
    pub device: &'a Rc<Device>,
    pub graphics_queue: vk::Queue,
    pub present_queue: vk::Queue,
    pub queue_family: QueueFamilyIndices,
    pub dimensions: [u32; 2],
    pub command_pool: vk::CommandPool,
    pub allocator: Rc<Mutex<Allocator>>,
    pub command_buffer_allocator: &'a CommandBufferAllocator,
    pub old_swapchain: Option<vk::SwapchainKHR>,
}

#[derive(Clone, TypedBuilder, Hash, PartialEq, Eq)]
pub struct FramebufferDescriptor {
    render_pass: vk::RenderPass,
    texture_views: Vec<vk::ImageView>,
    swapchain_extent: vk::Extent2D,
}

impl Swapchain {
    pub fn raw(&self) -> vk::SwapchainKHR {
        self.raw
    }

    pub fn loader(&self) -> &khr::Swapchain {
        &self.loader
    }

    pub fn surface_format(&self) -> vk::SurfaceFormatKHR {
        self.surface_format
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    pub fn render_pass(&self) -> &RenderPass {
        &self.render_pass
    }

    pub fn pipeline(&self) -> &Pipeline {
        &self.pipeline
    }

    pub fn new(desc: &SwapchainDescriptor) -> Result<Self, DeviceError> {
        let device = desc.device;
        let (swapchain_loader, swapchain, properties, support) = Self::create_swapchain(
            desc.adapter,
            desc.surface,
            desc.instance,
            device,
            &desc.queue_family,
            desc.dimensions,
            desc.old_swapchain,
        )?;
        let extent = properties.extent;
        // ?????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????
        let swapchain_textures = unsafe { swapchain_loader.get_swapchain_images(swapchain)? };

        let mut capabilities = support.capabilities;

        capabilities.current_extent.width = capabilities.current_extent.width.max(1);
        capabilities.current_extent.height = capabilities.current_extent.height.max(1);

        let texture_views = swapchain_textures
            .iter()
            .map(|i| {
                TextureView::new_color_texture_view(
                    Some("swapchain texture view"),
                    device,
                    *i,
                    properties.surface_format.format,
                )
                .unwrap()
            })
            .collect::<Vec<TextureView>>();

        // let memory_properties = unsafe {
        //     desc.instance
        //         .raw()
        //         .get_physical_device_memory_properties(desc.adapter.raw())
        // };

        let depth_format = Self::get_depth_format(desc.instance.raw(), desc.adapter.raw())?;

        let texture_desc = TextureDescriptor {
            device,
            image_type: vk::ImageType::TYPE_2D,
            format: depth_format,
            dimension: [extent.width, extent.height],
            mip_levels: 4,
            array_layers: 1,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            properties: vk::MemoryPropertyFlags::DEVICE_LOCAL,
            allocator: desc.allocator.clone(),
        };
        let depth_texture = Texture::new(texture_desc)?;

        let depth_texture_view = TextureView::new_depth_texture_view(
            Some("depth"),
            &device,
            depth_texture.raw(),
            depth_format,
        )?;

        let clear_color = Color::new(0.65, 0.8, 0.9, 1.0);
        let rect2d = math::Rect2D {
            x: 0.0,
            y: 0.0,
            width: extent.width as f32,
            height: extent.height as f32,
        };
        let map = Default::default();

        let render_pass_desc = RenderPassDescriptor {
            device,
            surface_format: properties.surface_format.format,
            depth_format,
            render_area: rect2d,
            clear_color,
            depth: 1.0,
            stencil: 0,
        };
        let render_pass = RenderPass::new(&render_pass_desc)?;

        let framebuffers = texture_views
            .iter()
            .map(|i| {
                let image_view = i.raw();
                let framebuffer_desc = FramebufferDescriptor::builder()
                    .texture_views(vec![image_view])
                    .swapchain_extent(extent)
                    .render_pass(render_pass.raw())
                    .build();
                Self::create_framebuffer(device, &map, framebuffer_desc)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let shader_desc = ShaderDescriptor {
            label: Some("Triangle"),
            device,
            vert_bytes: &Shader::load_pre_compiled_spv_bytes_from_name("triangle_0.vert"),
            vert_entry_name: "main",
            frag_bytes: &Shader::load_pre_compiled_spv_bytes_from_name("triangle_0.frag"),
            frag_entry_name: "main",
        };
        let shader = Shader::new(&shader_desc).map_err(|e| DeviceError::Other("Shader Error"))?;

        let pipeline = Pipeline::new(device, render_pass.raw(), shader)?;

        let command_buffers = desc
            .command_buffer_allocator
            .allocate_command_buffers(true, texture_views.len() as u32)?;

        // // ????????? vk::PhysicalDeviceMemoryProperties ????????????????????????????????????????????????????????????????????????????????????
        // // ????????? VRAM ?????? VRAM ????????? RAM ?????????????????????????????????????????????????????????????????????????????????????????????????????????
        // // ???????????????????????????????????????????????????????????????????????????
        // let mem_properties = {
        //     // profiling::scope!("vkGetPhysicalDeviceMemoryProperties");
        //     instance_fp.get_physical_device_memory_properties(self.raw)
        // };
        // // ??????????????????????????????????????????????????????
        // // ?????? requirements ?????????????????????????????????????????????????????????????????????????????????
        // // ??????????????????????????????????????????????????????????????????????????????????????? 1 ??????????????????????????????????????????
        // let memory_types =
        //     &mem_properties.memory_types[..mem_properties.memory_type_count as usize];
        // let valid_memory_types: u32 = memory_types.iter().enumerate().fold(0, |u, (i, mem)| {
        //     if self.known_memory_flags.contains(mem.property_flags) {
        //         u | (1 << i)
        //     } else {
        //         u
        //     }
        // });
        // let swapchain_loader = khr::Swapchain::new(&instance_fp, &ash_device);
        // let queue_family_index = indices.graphics_family.unwrap();
        // let raw_queue = {
        //     profiling::scope!("vkGetDeviceQueue");
        //     // queueFamilyIndex is the index of the queue family to which the queue belongs.
        //     // queueIndex is the index within this queue family of the queue to retrieve.
        //     ash_device.get_device_queue(queue_family_index, 0)
        // };
        Ok(Self {
            raw: swapchain,
            loader: swapchain_loader,
            device: desc.device.clone(),
            family_index: desc.queue_family,
            textures: swapchain_textures,
            surface_format: properties.surface_format,
            depth_format,
            extent: properties.extent,
            capabilities,
            texture_views,
            framebuffers,
            render_pass,
            pipeline,
            command_buffers,
            graphics_queue: desc.graphics_queue,
            present_queue: desc.present_queue,
            command_buffer_allocator: desc.command_buffer_allocator.clone(),
            depth_texture,
            depth_texture_view,
        })
    }

    pub fn render(&mut self, image_index: usize) -> Result<vk::CommandBuffer, DeviceError> {
        let command_buffer = &self.command_buffers[image_index];
        let framebuffer = self.framebuffers[image_index];

        self.device
            .reset_command_buffer(command_buffer.raw(), vk::CommandBufferResetFlags::empty())?;
        self.device.begin_command_buffer(
            command_buffer.raw(),
            &vk::CommandBufferBeginInfo::builder().build(),
        )?;

        self.render_pass.begin(command_buffer, framebuffer);
        self.device.cmd_bind_pipeline(
            command_buffer.raw(),
            vk::PipelineBindPoint::GRAPHICS,
            self.pipeline.raw(),
        );
        // ????????????????????? NDC
        let viewport_rect2d = math::Rect2D {
            x: 0.0,
            y: self.extent.height as f32,
            width: self.extent.width as f32,
            height: -(self.extent.height as f32),
        };
        self.device
            .cmd_set_viewport(command_buffer.raw(), viewport_rect2d);

        let scissor_rect2d = math::Rect2D {
            x: 0.0,
            y: 0.0,
            width: self.extent.width as f32,
            height: self.extent.height as f32,
        };
        self.device.cmd_set_scissor(
            command_buffer.raw(),
            0,
            &[conv::convert_rect2d(scissor_rect2d)],
        );

        self.device.cmd_draw(command_buffer.raw(), 3, 1, 0, 0);
        self.render_pass.end(command_buffer);
        self.device.end_command_buffer(command_buffer.raw())?;

        Ok(command_buffer.raw())
    }

    pub fn update_submitted_command_buffer(&mut self, command_buffer_index: usize) {
        let command_buffer = &mut self.command_buffers[command_buffer_index];
        command_buffer.set_state(CommandBufferState::Submitted);
    }

    fn create_swapchain(
        adapter: &Adapter,
        surface: &Surface,
        instance: &Instance,
        device: &Device,
        queue_family: &QueueFamilyIndices,
        dimensions: [u32; 2],
        old_swapchain: Option<vk::SwapchainKHR>,
    ) -> Result<
        (
            khr::Swapchain,
            vk::SwapchainKHR,
            SwapchainProperties,
            SwapChainSupportDetail,
        ),
        DeviceError,
    > {
        profiling::scope!("create_swapchain");

        let swapchain_support =
            unsafe { SwapChainSupportDetail::new(adapter.raw(), surface.loader(), surface.raw()) }?;
        let properties = swapchain_support.get_ideal_swapchain_properties(dimensions);
        let SwapchainProperties {
            surface_format,
            present_mode,
            extent,
        } = properties;

        let image_count = swapchain_support.capabilities.min_image_count + 1;
        let image_count = if swapchain_support.capabilities.max_image_count > 0 {
            image_count.min(swapchain_support.capabilities.max_image_count)
        } else {
            image_count
        };
        let (image_sharing_mode, queue_family_indices) =
            if queue_family.graphics_family != queue_family.present_family {
                (
                    // ????????????????????????????????????????????????????????????????????????????????????
                    // ???????????????????????????????????????????????????????????????????????????????????????????????????????????????
                    vk::SharingMode::CONCURRENT,
                    vec![
                        queue_family.graphics_family.unwrap(),
                        queue_family.present_family.unwrap(),
                    ],
                )
            } else {
                // ???????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????
                // ????????????????????????????????????
                (vk::SharingMode::EXCLUSIVE, vec![])
            };

        let old_swapchain = match old_swapchain {
            None => vk::SwapchainKHR::null(),
            Some(swapchain) => swapchain,
        };

        let create_info = vk::SwapchainCreateInfoKHR::builder()
            .surface(surface.raw())
            .min_image_count(image_count)
            .image_color_space(surface_format.color_space)
            .image_format(surface_format.format)
            .image_extent(extent)
            // ?????????????????????????????????
            // ??????????????????????????????????????? TRANSFER_DST??????????????? image ????????????????????????
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(image_sharing_mode)
            .queue_family_indices(&queue_family_indices)
            // ????????????????????????????????????????????????????????? 90 ???????????????????????????????????????????????????
            .pre_transform(swapchain_support.capabilities.current_transform)
            // ?????? alpha ???????????????????????????????????????????????????????????????????????????
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            // ???????????????????????????????????????????????????????????????????????????????????? Vulkan ???
            // ??????????????????????????????????????????????????????????????????????????????????????????
            .clipped(true)
            // ?????? VR ?????????????????????????????????????????????????????????
            .image_array_layers(1)
            .old_swapchain(old_swapchain);

        let swapchain_loader = khr::Swapchain::new(instance.raw(), device.raw());
        let swapchain = unsafe { swapchain_loader.create_swapchain(&create_info, None)? };
        log::debug!("Vulkan swapchain created.");

        Ok((swapchain_loader, swapchain, properties, swapchain_support))
    }

    pub fn create_framebuffer(
        device: &Device,
        map: &Mutex<fxhash::FxHashMap<FramebufferDescriptor, vk::Framebuffer>>,
        desc: FramebufferDescriptor,
    ) -> Result<vk::Framebuffer, DeviceError> {
        use std::collections::hash_map::Entry;
        Ok(match map.lock().entry(desc) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                let desc = e.key();
                let create_info = vk::FramebufferCreateInfo::builder()
                    .render_pass(desc.render_pass)
                    .attachments(&desc.texture_views)
                    .width(desc.swapchain_extent.width)
                    .height(desc.swapchain_extent.height)
                    .layers(1)
                    .build();
                device.create_framebuffer(&create_info)?
            }
        })
    }

    pub fn acquire_next_image(
        &self,
        timeout: u64,
        semaphore: vk::Semaphore,
    ) -> Result<(u32, bool), SurfaceError> {
        match unsafe {
            self.loader
                .acquire_next_image(self.raw, timeout, semaphore, vk::Fence::null())
        } {
            Ok(pair) => Ok(pair),
            Err(error) => match error {
                vk::Result::ERROR_OUT_OF_DATE_KHR | vk::Result::NOT_READY => {
                    Err(SurfaceError::OutOfDate)
                }
                vk::Result::ERROR_SURFACE_LOST_KHR => Err(SurfaceError::Lost),
                other => Err(DeviceError::from(other).into()),
            },
        }
    }

    pub fn queue_present(&self, present_info: &vk::PresentInfoKHR) -> Result<bool, SurfaceError> {
        match unsafe { self.loader.queue_present(self.present_queue, present_info) } {
            Ok(suboptimal) => Ok(suboptimal),
            Err(error) => match error {
                vk::Result::ERROR_OUT_OF_DATE_KHR | vk::Result::NOT_READY => {
                    Err(SurfaceError::OutOfDate)
                }
                vk::Result::ERROR_SURFACE_LOST_KHR => Err(SurfaceError::Lost),
                other => Err(DeviceError::from(other).into()),
            },
        }
    }

    fn get_depth_format(
        instance: &ash::Instance,
        adapter: vk::PhysicalDevice,
    ) -> Result<vk::Format, DeviceError> {
        let formats = &[
            vk::Format::D32_SFLOAT,
            vk::Format::D32_SFLOAT_S8_UINT,
            vk::Format::D24_UNORM_S8_UINT,
        ];

        Texture::get_supported_format(
            instance,
            adapter,
            formats,
            vk::ImageTiling::OPTIMAL,
            vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT,
        )
    }

    pub fn get_memory_type_index(
        memory_properties: vk::PhysicalDeviceMemoryProperties,
        properties: vk::MemoryPropertyFlags,
        requirements: vk::MemoryRequirements,
    ) -> u32 {
        // ??????????????????????????????????????????????????????
        // ?????? requirements ?????????????????????????????????????????????????????????????????????????????????
        // ??????????????????????????????????????????????????????????????????????????????????????? 1 ??????????????????????????????????????????

        // ?????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????
        // memory_types ????????? vk::MemoryType ???????????????????????????????????????????????????????????????????????????????????????????????????
        // ????????????????????????????????????????????? CPU ??????????????????????????? vk::MemoryPropertyFlags::HOST_VISIBLE ?????????
        // ??????????????????????????? vk::MemoryPropertyFlags::HOST_COHERENT ???????????????????????????????????????????????????
        (0..memory_properties.memory_type_count)
            .find(|i| {
                let suitable = (requirements.memory_type_bits & (1 << i)) != 0;
                let memory_type = memory_properties.memory_types[*i as usize];
                suitable && memory_type.property_flags.contains(properties)
            })
            .expect("Failed to find suitable memory type!")
    }
}

impl SwapChainSupportDetail {
    pub unsafe fn new(
        physical_device: vk::PhysicalDevice,
        surface: &khr::Surface,
        surface_khr: vk::SurfaceKHR,
    ) -> Result<SwapChainSupportDetail, DeviceError> {
        let capabilities =
            surface.get_physical_device_surface_capabilities(physical_device, surface_khr)?;
        let surface_formats =
            surface.get_physical_device_surface_formats(physical_device, surface_khr)?;
        let present_modes =
            surface.get_physical_device_surface_present_modes(physical_device, surface_khr)?;

        Ok(SwapChainSupportDetail {
            capabilities,
            surface_formats,
            present_modes,
        })
    }

    pub fn get_ideal_swapchain_properties(
        &self,
        preferred_dimensions: [u32; 2],
    ) -> SwapchainProperties {
        let format = Self::choose_swapchain_format(&self.surface_formats);
        let present_mode = Self::choose_swapchain_present_mode(&self.present_modes);
        let extent = Self::choose_swapchain_extent(&self.capabilities, preferred_dimensions);
        SwapchainProperties {
            surface_format: format,
            present_mode,
            extent,
        }
    }

    fn choose_swapchain_format(
        available_formats: &Vec<vk::SurfaceFormatKHR>,
    ) -> vk::SurfaceFormatKHR {
        // check if list contains most widely used R8G8B8A8 format with nonlinear color space
        for available_format in available_formats {
            if available_format.format == vk::Format::B8G8R8A8_SRGB
                && available_format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            {
                return *available_format;
            }
        }

        // return the first format from the list
        return *available_formats.first().unwrap();
    }

    fn choose_swapchain_present_mode(
        available_present_modes: &[vk::PresentModeKHR],
    ) -> vk::PresentModeKHR {
        // ?????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????????
        // ??????????????????????????????????????????????????????????????????????????????????????????????????????
        // ????????????????????????????????????????????? VK_PRESENT_MODE_FIFO_KHR???????????????????????????????????????????????????
        // VK_PRESENT_MODE_IMMEDIATE_KHR ?????? VK_PRESENT_MODE_MAILBOX_KHR??? VK_PRESENT_MODE_IMMEDIATE_KHR
        // ??????????????????????????????????????????????????????????????????????????????????????? VK_PRESENT_MODE_MAILBOX_KHR
        // ???????????????????????????????????????????????????????????????????????????????????????????????????
        let mut best_mode = vk::PresentModeKHR::FIFO;
        for &available_present_mode in available_present_modes.iter() {
            if available_present_mode == vk::PresentModeKHR::MAILBOX {
                return available_present_mode;
            } else if available_present_mode == vk::PresentModeKHR::IMMEDIATE {
                // ?????????????????????????????????????????? FIFO ?????????????????????????????????
                // ??????????????? Mailbox ?????????????????????????????????????????? IMMEDIATE ??????
                best_mode = vk::PresentModeKHR::IMMEDIATE;
            }
        }

        best_mode
    }

    fn choose_swapchain_extent(
        capabilities: &vk::SurfaceCapabilitiesKHR,
        preferred_dimensions: [u32; 2],
    ) -> vk::Extent2D {
        if capabilities.current_extent.width != u32::MAX {
            capabilities.current_extent
        } else {
            use num::clamp;
            let width = preferred_dimensions[0];
            let height = preferred_dimensions[1];
            log::debug!("\t\tInner Window Size: ({}, {})", width, height);
            vk::Extent2D {
                width: clamp(
                    width,
                    capabilities.min_image_extent.width,
                    capabilities.max_image_extent.width,
                ),
                height: clamp(
                    height,
                    capabilities.min_image_extent.height,
                    capabilities.max_image_extent.height,
                ),
            }
        }
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        self.framebuffers
            .iter()
            .for_each(|e| self.device.destroy_framebuffer(*e));

        unsafe {
            self.loader.destroy_swapchain(self.raw, None);
        }
        log::debug!("Swapchain destroyed.");
    }
}
