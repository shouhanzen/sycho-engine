//! Private IOSurface handoff between the editor and its Metal preview process.

use std::collections::HashSet;
use std::ffi::{CString, c_char, c_void};
use std::io;
use std::ptr::NonNull;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use core_foundation::{
    base::{CFRelease, TCFType},
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::{CFString, CFStringRef},
};
use foreign_types::ForeignType;
use mach2::{
    mach_port::{
        mach_port_allocate, mach_port_deallocate, mach_port_insert_right, mach_port_mod_refs,
    },
    message::*,
    port::*,
    traps::mach_task_self,
};
use objc::runtime::{Object, Sel};
use pixels::wgpu;
use wgpu_hal::{api::Metal, metal::Device as MetalDevice};

use crate::surface::SurfaceSize;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
const PIXEL_FORMAT: u32 = u32::from_be_bytes(*b"BGRA");
const MESSAGE_TIMEOUT_MS: u32 = 2_000;

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    static kIOSurfaceWidth: CFStringRef;
    static kIOSurfaceHeight: CFStringRef;
    static kIOSurfaceBytesPerElement: CFStringRef;
    static kIOSurfacePixelFormat: CFStringRef;
    fn IOSurfaceCreate(properties: CFDictionaryRef) -> *mut c_void;
    fn IOSurfaceCreateMachPort(surface: *mut c_void) -> mach_port_t;
    fn IOSurfaceLookupFromMachPort(port: mach_port_t) -> *mut c_void;
    fn IOSurfaceGetID(surface: *mut c_void) -> u32;
    fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
    fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
    fn IOSurfaceGetPixelFormat(surface: *mut c_void) -> u32;
    fn IOSurfaceGetBytesPerElement(surface: *mut c_void) -> usize;
}

unsafe extern "C" {
    static bootstrap_port: mach_port_t;
    fn bootstrap_register(bootstrap: mach_port_t, name: *const c_char, port: mach_port_t) -> i32;
    fn bootstrap_look_up(
        bootstrap: mach_port_t,
        name: *const c_char,
        port: *mut mach_port_t,
    ) -> i32;
    fn arc4random_buf(buffer: *mut c_void, size: usize);
    fn mach_msg_destroy(header: *mut mach_msg_header_t);
}

pub struct Surface(NonNull<c_void>);

// IOSurface references are retained across threads. The preview lease below
// orders GPU access across processes before a buffer can be reused.
unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

impl Surface {
    pub(crate) fn new(size: SurfaceSize) -> io::Result<Self> {
        let properties = unsafe {
            CFDictionary::from_CFType_pairs(&[
                (
                    CFString::wrap_under_get_rule(kIOSurfaceWidth),
                    CFNumber::from(i64::from(size.width)),
                ),
                (
                    CFString::wrap_under_get_rule(kIOSurfaceHeight),
                    CFNumber::from(i64::from(size.height)),
                ),
                (
                    CFString::wrap_under_get_rule(kIOSurfaceBytesPerElement),
                    CFNumber::from(4i64),
                ),
                (
                    CFString::wrap_under_get_rule(kIOSurfacePixelFormat),
                    CFNumber::from(i64::from(PIXEL_FORMAT)),
                ),
            ])
        };
        NonNull::new(unsafe { IOSurfaceCreate(properties.as_concrete_TypeRef()) })
            .map(Self)
            .ok_or_else(|| io::Error::other("could not allocate a preview IOSurface"))
    }

    pub fn id(&self) -> u32 {
        unsafe { IOSurfaceGetID(self.0.as_ptr()) }
    }

    fn validate(&self, size: SurfaceSize) -> io::Result<()> {
        let raw = self.0.as_ptr();
        if unsafe {
            IOSurfaceGetWidth(raw) == size.width as usize
                && IOSurfaceGetHeight(raw) == size.height as usize
                && IOSurfaceGetPixelFormat(raw) == PIXEL_FORMAT
                && IOSurfaceGetBytesPerElement(raw) == 4
        } {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "preview IOSurface format or size mismatch",
            ))
        }
    }

    pub fn sampled_texture(
        &self,
        device: &wgpu::Device,
        size: SurfaceSize,
    ) -> io::Result<wgpu::Texture> {
        self.texture(device, size, wgpu::TextureUsages::TEXTURE_BINDING)
    }

    pub(crate) fn render_texture(
        &self,
        device: &wgpu::Device,
        size: SurfaceSize,
    ) -> io::Result<wgpu::Texture> {
        self.texture(device, size, wgpu::TextureUsages::RENDER_ATTACHMENT)
    }

    fn texture(
        &self,
        device: &wgpu::Device,
        size: SurfaceSize,
        usage: wgpu::TextureUsages,
    ) -> io::Result<wgpu::Texture> {
        self.validate(size)?;
        if size.width.max(size.height) > device.limits().max_texture_dimension_2d {
            return Err(io::Error::other(
                "preview IOSurface exceeds the texture size limit",
            ));
        }
        let metal_usage = if usage == wgpu::TextureUsages::RENDER_ATTACHMENT {
            metal::MTLTextureUsage::RenderTarget
        } else {
            metal::MTLTextureUsage::ShaderRead
        };
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_pixel_format(metal::MTLPixelFormat::BGRA8Unorm_sRGB);
        descriptor.set_texture_type(metal::MTLTextureType::D2);
        descriptor.set_width(size.width.into());
        descriptor.set_height(size.height.into());
        descriptor.set_storage_mode(metal::MTLStorageMode::Shared);
        descriptor.set_usage(metal_usage);
        // as_hal holds the wgpu device lock. Only call raw Metal inside it.
        let raw = unsafe {
            device.as_hal::<Metal, _, _>(|hal| {
                let hal = hal?;
                let metal = hal.raw_device().lock();
                let selector = Sel::register("newTextureWithDescriptor:iosurface:plane:");
                let raw: *mut metal::MTLTexture = objc::__send_message(
                    &**metal as *const metal::DeviceRef as *const Object,
                    selector,
                    (descriptor.as_ptr(), self.0.as_ptr(), 0usize),
                )
                .ok()?;
                NonNull::new(raw).map(|raw| metal::Texture::from_ptr(raw.as_ptr()))
            })
        }
        .ok_or_else(|| io::Error::other("Metal could not import the preview IOSurface"))?;
        let extent = wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        };
        // Both wrappers describe the same one-plane Metal texture. The producer
        // clears the entire attachment on its first use and publishes after GPU
        // completion. The editor imports only completed surfaces.
        Ok(unsafe {
            let texture = MetalDevice::texture_from_raw(
                raw,
                FORMAT,
                metal::MTLTextureType::D2,
                1,
                1,
                wgpu_hal::CopyExtent {
                    width: size.width,
                    height: size.height,
                    depth: 1,
                },
            );
            device.create_texture_from_hal::<Metal>(
                texture,
                &wgpu::TextureDescriptor {
                    label: Some("shared preview IOSurface"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage,
                    view_formats: &[],
                },
            )
        })
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0.as_ptr()) };
    }
}

pub fn device_id(device: &wgpu::Device) -> Option<u64> {
    unsafe {
        device
            .as_hal::<Metal, _, _>(|hal| hal.map(|device| device.raw_device().lock().registry_id()))
    }
}

pub(crate) fn adapter_for_device(instance: &wgpu::Instance, id: u64) -> Option<wgpu::Adapter> {
    let metal = metal::Device::all()
        .into_iter()
        .find(|device| device.registry_id() == id)?;
    instance
        .enumerate_adapters(wgpu::Backends::METAL)
        .find(|adapter| adapter.get_info().name == metal.name())
}

struct SendRight(mach_port_t);

impl Drop for SendRight {
    fn drop(&mut self) {
        unsafe { mach_port_deallocate(mach_task_self(), self.0) };
    }
}

#[repr(C)]
#[derive(Default)]
struct SurfaceMessage {
    header: mach_msg_header_t,
    body: mach_msg_body_t,
    surface: mach_msg_port_descriptor_t,
}

#[repr(C)]
struct ReceiveSurfaceMessage {
    message: SurfaceMessage,
    // Mach can append a receive trailer after the message.
    trailer: [u32; 32],
}

pub struct SurfaceInbox(mach_port_t);

impl SurfaceInbox {
    pub fn open(device: u64) -> io::Result<(Self, String)> {
        let mut secret = [0u8; 16];
        unsafe { arc4random_buf(secret.as_mut_ptr().cast(), secret.len()) };
        let name = CString::new(format!("org.sycho.preview.{}", hex::encode(secret))).unwrap();
        let mut port = MACH_PORT_NULL;
        // Keep a normal receive queue under our control. The name only serves
        // as a short-lived rendezvous to give the child a send right to it.
        let task = unsafe { mach_task_self() };
        mach_result("allocate preview inbox", unsafe {
            mach_port_allocate(task, MACH_PORT_RIGHT_RECEIVE, &mut port)
        })?;
        let inbox = Self(port);
        mach_result("create preview inbox send right", unsafe {
            mach_port_insert_right(task, port, port, MACH_MSG_TYPE_MAKE_SEND)
        })?;
        let send = SendRight(port);
        mach_result("register preview inbox", unsafe {
            bootstrap_register(bootstrap_port, name.as_ptr(), send.0)
        })?;
        Ok((inbox, format!("{device}:{}", name.to_str().unwrap())))
    }

    pub fn receive(&self, id: u32, size: SurfaceSize) -> io::Result<Surface> {
        let mut buffer = ReceiveSurfaceMessage {
            message: SurfaceMessage::default(),
            trailer: [0; 32],
        };
        mach_result("receive preview surface", unsafe {
            mach_msg(
                &mut buffer.message.header,
                MACH_RCV_MSG | MACH_RCV_TIMEOUT,
                0,
                size_of::<ReceiveSurfaceMessage>() as u32,
                self.0,
                MESSAGE_TIMEOUT_MS,
                MACH_PORT_NULL,
            )
        })?;
        let message = &mut buffer.message;
        if message.header.msgh_bits & MACH_MSGH_BITS_COMPLEX == 0
            || message.header.msgh_size != size_of::<SurfaceMessage>() as u32
            || message.header.msgh_id as u32 != id
            || message.body.msgh_descriptor_count != 1
            || message.surface.type_ != MACH_MSG_PORT_DESCRIPTOR as u8
            || message.surface.disposition != MACH_MSG_TYPE_MOVE_SEND as u8
        {
            unsafe { mach_msg_destroy(&mut message.header) };
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid preview surface message",
            ));
        }
        let port = SendRight(message.surface.name);
        let surface = NonNull::new(unsafe { IOSurfaceLookupFromMachPort(port.0) })
            .map(Surface)
            .ok_or_else(|| io::Error::other("invalid preview IOSurface right"))?;
        surface.validate(size)?;
        Ok(surface)
    }
}

impl Drop for SurfaceInbox {
    fn drop(&mut self) {
        unsafe { mach_port_mod_refs(mach_task_self(), self.0, MACH_PORT_RIGHT_RECEIVE, -1) };
    }
}

pub(crate) struct SurfaceSender {
    port: SendRight,
    pub device: u64,
}

impl SurfaceSender {
    pub fn open(config: &str) -> io::Result<Self> {
        let (device, name) = config
            .split_once(':')
            .ok_or_else(|| io::Error::other("invalid surface endpoint"))?;
        let device = device.parse().map_err(io::Error::other)?;
        let name = CString::new(name)?;
        let mut port = MACH_PORT_NULL;
        mach_result("look up preview inbox", unsafe {
            bootstrap_look_up(bootstrap_port, name.as_ptr(), &mut port)
        })?;
        Ok(Self {
            port: SendRight(port),
            device,
        })
    }

    pub fn send(&self, id: u32, surface: &Surface) -> io::Result<()> {
        let port = SendRight(unsafe { IOSurfaceCreateMachPort(surface.0.as_ptr()) });
        if port.0 == MACH_PORT_NULL {
            return Err(io::Error::other("could not share preview IOSurface"));
        }
        let mut message = SurfaceMessage {
            header: mach_msg_header_t {
                msgh_bits: MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, 0) | MACH_MSGH_BITS_COMPLEX,
                msgh_size: size_of::<SurfaceMessage>() as u32,
                msgh_remote_port: self.port.0,
                msgh_id: id as i32,
                ..Default::default()
            },
            body: mach_msg_body_t {
                msgh_descriptor_count: 1,
            },
            surface: mach_msg_port_descriptor_t::new(port.0, MACH_MSG_TYPE_COPY_SEND),
        };
        mach_result("send preview surface", unsafe {
            mach_msg(
                &mut message.header,
                MACH_SEND_MSG | MACH_SEND_TIMEOUT,
                size_of::<SurfaceMessage>() as u32,
                0,
                MACH_PORT_NULL,
                MESSAGE_TIMEOUT_MS,
                MACH_PORT_NULL,
            )
        })
    }
}

fn mach_result(operation: &str, result: i32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        let kind = if matches!(result, MACH_SEND_INVALID_DEST | MACH_RCV_PORT_DIED) {
            io::ErrorKind::BrokenPipe
        } else {
            io::ErrorKind::Other
        };
        Err(io::Error::new(kind, format!("{operation}: Mach error {result:#x}")))
    }
}

#[derive(Default)]
pub(crate) struct SurfaceLeases {
    disabled: AtomicBool,
    busy: Mutex<HashSet<u32>>,
}

impl SurfaceLeases {
    pub fn enabled(&self) -> bool {
        !self.disabled.load(Ordering::Relaxed)
    }
    pub fn disable(&self) {
        self.disabled.store(true, Ordering::Relaxed);
    }
    pub fn acquire(&self, id: u32) -> bool {
        self.busy.lock().unwrap().insert(id)
    }
    pub fn release(&self, id: u32) {
        self.busy.lock().unwrap().remove(&id);
    }
}
