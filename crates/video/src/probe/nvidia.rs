//! The NVIDIA side of the probe: CUDA's driver API (compute capability,
//! contexts, external memory), NVENC (API version, codecs, limits) and
//! NVDEC (decoder limits), each from the driver's own library, opened at
//! run time. GPUs are matched to Vulkan's by device UUID.
//!
//! The declarations follow FFmpeg's nv-codec-headers n12.1.14.1 (NVENC API
//! 12.1, the human's choice: drivers from Linux 530.41 and Windows 531.61):
//! `dynlink_cuda.h`, `nvEncodeAPI.h`, `dynlink_cuviddec.h` and the library
//! and symbol names of `dynlink_loader.h`. Line numbers are cited where a
//! value is used.
//!
//! `unsafe`: FFI into those libraries. Every structure passed is
//! `#[repr(C)]` with the header's layout (sizes checked in the tests), lives
//! until the call returns, and is zeroed where the header asks for reserved
//! fields to be 0. Contexts and sessions are released by guards.

use std::ffi::{c_int, c_uint, c_void};

use libloading::Library;

use super::{Gpu, NvdecCodecCaps, NvencCodecCaps, Nvidia, Vendor};

type CuResult = c_int;
type CuDevice = c_int;
type CuContext = *mut c_void;
type CuDevicePtr = u64;
type CuExternalMemory = *mut c_void;
type NvEncStatus = c_int;

const CUDA_SUCCESS: CuResult = 0; // dynlink_cuda.h:67
const NV_ENC_SUCCESS: NvEncStatus = 0; // nvEncodeAPI.h:501
const CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR: c_int = 75; // dynlink_cuda.h:94
const CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR: c_int = 76; // dynlink_cuda.h:95

#[cfg(target_os = "linux")]
mod names {
    // dynlink_loader.h:58-60
    pub const CUDA: &str = "libcuda.so.1";
    pub const NVENC: &str = "libnvidia-encode.so.1";
    pub const NVCUVID: &str = "libnvcuvid.so.1";
}
#[cfg(target_os = "windows")]
mod names {
    // dynlink_loader.h:50-53 (64-bit)
    pub const CUDA: &str = "nvcuda.dll";
    pub const NVENC: &str = "nvEncodeAPI64.dll";
    pub const NVCUVID: &str = "nvcuvid.dll";
}

/// Opens `name` and takes its symbols with `f`; `None` when the library or
/// a symbol is missing.
fn open<T>(name: &str, f: impl FnOnce(&Library) -> Option<T>) -> Option<(Library, T)> {
    // SAFETY: the NVIDIA driver's libraries run no code on load that needs
    // anything from us.
    let lib = unsafe { Library::new(name) }.ok()?;
    let fns = f(&lib)?;
    Some((lib, fns))
}

/// One symbol as a function pointer of type `T`.
///
/// # Safety
/// `T` must be the symbol's real type.
unsafe fn symbol<T: Copy>(lib: &Library, name: &[u8]) -> Option<T> {
    // SAFETY: the caller promises the type.
    unsafe { lib.get::<T>(name) }.ok().map(|s| *s)
}

// --- CUDA driver API (dynlink_cuda.h:435-526) ---------------------------

#[repr(C)]
#[derive(Clone, Copy)]
struct CuUuid {
    bytes: [u8; 16], // dynlink_cuda.h:241-243
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Win32Handle {
    handle: *mut c_void,
    name: *const c_void,
}

#[repr(C)]
union ExternalHandle {
    fd: c_int,
    win32: Win32Handle,
}

/// CUDA_EXTERNAL_MEMORY_HANDLE_DESC (dynlink_cuda.h:328-340).
#[repr(C)]
struct ExternalMemoryHandleDesc {
    kind: c_uint,
    handle: ExternalHandle,
    size: u64,
    flags: c_uint,
    reserved: [c_uint; 16],
}

/// CUDA_EXTERNAL_MEMORY_BUFFER_DESC (dynlink_cuda.h:342-347).
#[repr(C)]
struct ExternalMemoryBufferDesc {
    offset: u64,
    size: u64,
    flags: c_uint,
    reserved: [c_uint; 16],
}

#[cfg(target_os = "linux")]
const CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD: c_uint = 1; // dynlink_cuda.h:166

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_os = "linux"),
    expect(
        dead_code,
        reason = "the memory functions serve the Linux-only Vulkan to CUDA check"
    )
)]
struct CudaFns {
    init: unsafe extern "system" fn(c_uint) -> CuResult,
    driver_get_version: unsafe extern "system" fn(*mut c_int) -> CuResult,
    device_get_count: unsafe extern "system" fn(*mut c_int) -> CuResult,
    device_get: unsafe extern "system" fn(*mut CuDevice, c_int) -> CuResult,
    device_get_attribute: unsafe extern "system" fn(*mut c_int, c_int, CuDevice) -> CuResult,
    device_get_uuid: unsafe extern "system" fn(*mut CuUuid, CuDevice) -> CuResult,
    ctx_create: unsafe extern "system" fn(*mut CuContext, c_uint, CuDevice) -> CuResult,
    ctx_destroy: unsafe extern "system" fn(CuContext) -> CuResult,
    mem_free: unsafe extern "system" fn(CuDevicePtr) -> CuResult,
    memcpy_htod: unsafe extern "system" fn(CuDevicePtr, *const c_void, usize) -> CuResult,
    memcpy_dtoh: unsafe extern "system" fn(*mut c_void, CuDevicePtr, usize) -> CuResult,
    import_external_memory: Option<
        unsafe extern "system" fn(
            *mut CuExternalMemory,
            *const ExternalMemoryHandleDesc,
        ) -> CuResult,
    >,
    destroy_external_memory: Option<unsafe extern "system" fn(CuExternalMemory) -> CuResult>,
    external_memory_get_mapped_buffer: Option<
        unsafe extern "system" fn(
            *mut CuDevicePtr,
            CuExternalMemory,
            *const ExternalMemoryBufferDesc,
        ) -> CuResult,
    >,
}

struct Cuda {
    _lib: Library,
    f: CudaFns,
}

impl Cuda {
    fn load() -> Option<Self> {
        let (lib, f) = open(names::CUDA, |lib| {
            // SAFETY: each type is the header's declaration of that symbol
            // (dynlink_loader.h:326-417 names them).
            unsafe {
                Some(CudaFns {
                    init: symbol(lib, b"cuInit\0")?,
                    driver_get_version: symbol(lib, b"cuDriverGetVersion\0")?,
                    device_get_count: symbol(lib, b"cuDeviceGetCount\0")?,
                    device_get: symbol(lib, b"cuDeviceGet\0")?,
                    device_get_attribute: symbol(lib, b"cuDeviceGetAttribute\0")?,
                    device_get_uuid: symbol(lib, b"cuDeviceGetUuid_v2\0")
                        .or_else(|| symbol(lib, b"cuDeviceGetUuid\0"))?,
                    ctx_create: symbol(lib, b"cuCtxCreate_v2\0")?,
                    ctx_destroy: symbol(lib, b"cuCtxDestroy_v2\0")?,
                    mem_free: symbol(lib, b"cuMemFree_v2\0")?,
                    memcpy_htod: symbol(lib, b"cuMemcpyHtoD_v2\0")?,
                    memcpy_dtoh: symbol(lib, b"cuMemcpyDtoH_v2\0")?,
                    import_external_memory: symbol(lib, b"cuImportExternalMemory\0"),
                    destroy_external_memory: symbol(lib, b"cuDestroyExternalMemory\0"),
                    external_memory_get_mapped_buffer: symbol(
                        lib,
                        b"cuExternalMemoryGetMappedBuffer\0",
                    ),
                })
            }
        })?;
        Some(Self { _lib: lib, f })
    }

    fn attribute(&self, attribute: c_int, device: CuDevice) -> Option<u32> {
        let mut value = 0;
        // SAFETY: `value` is a valid out pointer; `device` came from
        // `cuDeviceGet`.
        let r = unsafe { (self.f.device_get_attribute)(&mut value, attribute, device) };
        (r == CUDA_SUCCESS)
            .then(|| u32::try_from(value).ok())
            .flatten()
    }

    /// A context on `device`, current on this thread until it is dropped.
    fn context(&self, device: CuDevice) -> Option<Context<'_>> {
        let mut ctx = std::ptr::null_mut();
        // SAFETY: `ctx` is a valid out pointer; flags 0 (automatic
        // scheduling).
        let r = unsafe { (self.f.ctx_create)(&mut ctx, 0, device) };
        (r == CUDA_SUCCESS && !ctx.is_null()).then_some(Context {
            cuda: self,
            raw: ctx,
        })
    }
}

/// A CUDA context, destroyed on drop.
struct Context<'a> {
    cuda: &'a Cuda,
    raw: CuContext,
}

impl Drop for Context<'_> {
    fn drop(&mut self) {
        // SAFETY: made by `cuCtxCreate_v2` and destroyed once; destroying
        // the current context also pops it.
        unsafe { (self.cuda.f.ctx_destroy)(self.raw) };
    }
}

// --- NVENC (nvEncodeAPI.h, API 12.1) --------------------------------------

/// The API version we are written against (nvEncodeAPI.h:118-121).
const NVENCAPI_VERSION: u32 = 12 | (1 << 24);

/// `NVENCAPI_STRUCT_VERSION(ver)` (nvEncodeAPI.h:126).
const fn struct_version(ver: u32) -> u32 {
    NVENCAPI_VERSION | (ver << 16) | (0x7 << 28)
}

const NV_ENC_DEVICE_TYPE_CUDA: u32 = 1; // nvEncodeAPI.h:797
// NV_ENC_CAPS, numbered from 0 (nvEncodeAPI.h:825 on).
const NV_ENC_CAPS_NUM_MAX_TEMPORAL_LAYERS: u32 = 10;
const NV_ENC_CAPS_LEVEL_MAX: u32 = 13;
const NV_ENC_CAPS_WIDTH_MAX: u32 = 16;
const NV_ENC_CAPS_HEIGHT_MAX: u32 = 17;
const NV_ENC_CAPS_SUPPORT_DYN_BITRATE_CHANGE: u32 = 20;
const NV_ENC_CAPS_MB_PER_SEC_MAX: u32 = 32;
const NV_ENC_CAPS_NUM_ENCODER_ENGINES: u32 = 49;

/// GUID (nvEncodeAPI.h:87-94).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// nvEncodeAPI.h:144-153.
const H264_GUID: Guid = Guid {
    data1: 0x6bc8_2762,
    data2: 0x4e63,
    data3: 0x4ca4,
    data4: [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf],
};
const HEVC_GUID: Guid = Guid {
    data1: 0x790c_dc88,
    data2: 0x4522,
    data3: 0x4d7b,
    data4: [0x94, 0x25, 0xbd, 0xa9, 0x97, 0x5f, 0x76, 0x03],
};
const AV1_GUID: Guid = Guid {
    data1: 0x0a35_2289,
    data2: 0x0aa7,
    data3: 0x4759,
    data4: [0x86, 0x2d, 0x5d, 0x15, 0xcd, 0x16, 0xd2, 0x54],
};

/// NV_ENCODE_API_FUNCTION_LIST (nvEncodeAPI.h:4374-4423): two `uint32_t`,
/// 43 function pointers (with `reserved1` among them) and 275 reserved
/// pointers. Only the slots below are called.
#[repr(C)]
struct FunctionList {
    version: u32,
    reserved: u32,
    fns: [*mut c_void; 43],
    reserved2: [*mut c_void; 275],
}

// Slots of `FunctionList::fns`, in the header's order.
const SLOT_GET_ENCODE_GUID_COUNT: usize = 1;
const SLOT_GET_ENCODE_GUIDS: usize = 4;
const SLOT_GET_ENCODE_CAPS: usize = 7;
const SLOT_DESTROY_ENCODER: usize = 27;
const SLOT_OPEN_ENCODE_SESSION_EX: usize = 29;

const NV_ENCODE_API_FUNCTION_LIST_VER: u32 = struct_version(2); // nvEncodeAPI.h:4426

/// NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS (nvEncodeAPI.h:2727-2736).
#[repr(C)]
struct OpenSessionParams {
    version: u32,
    device_type: u32,
    device: *mut c_void,
    reserved: *mut c_void,
    api_version: u32,
    reserved1: [u32; 253],
    reserved2: [*mut c_void; 64],
}

const NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER: u32 = struct_version(1); // :2738

/// NV_ENC_CAPS_PARAM (nvEncodeAPI.h:1293-1298).
#[repr(C)]
struct CapsParam {
    version: u32,
    caps_to_query: u32,
    reserved: [u32; 62],
}

const NV_ENC_CAPS_PARAM_VER: u32 = struct_version(1); // :1302

type GetGuidCount = unsafe extern "system" fn(*mut c_void, *mut u32) -> NvEncStatus;
type GetGuids = unsafe extern "system" fn(*mut c_void, *mut Guid, u32, *mut u32) -> NvEncStatus;
type GetCaps =
    unsafe extern "system" fn(*mut c_void, Guid, *mut CapsParam, *mut c_int) -> NvEncStatus;
type DestroyEncoder = unsafe extern "system" fn(*mut c_void) -> NvEncStatus;
type OpenSession =
    unsafe extern "system" fn(*mut OpenSessionParams, *mut *mut c_void) -> NvEncStatus;

struct Nvenc {
    _lib: Library,
    max_version: u32,
    list: Box<FunctionList>,
}

impl Nvenc {
    fn load() -> Option<Self> {
        type MaxVersion = unsafe extern "system" fn(*mut u32) -> NvEncStatus;
        type CreateInstance = unsafe extern "system" fn(*mut FunctionList) -> NvEncStatus;
        let (lib, (max, create)) = open(names::NVENC, |lib| {
            // SAFETY: the header's declarations (nvEncodeAPI.h:4272, 4443;
            // names from dynlink_loader.h:490-491).
            unsafe {
                Some((
                    symbol::<MaxVersion>(lib, b"NvEncodeAPIGetMaxSupportedVersion\0")?,
                    symbol::<CreateInstance>(lib, b"NvEncodeAPICreateInstance\0")?,
                ))
            }
        })?;
        let mut max_version = 0;
        // SAFETY: a valid out pointer.
        if unsafe { max(&mut max_version) } != NV_ENC_SUCCESS {
            return None;
        }
        let mut list = Box::new(FunctionList {
            version: NV_ENCODE_API_FUNCTION_LIST_VER,
            reserved: 0,
            fns: [std::ptr::null_mut(); 43],
            reserved2: [std::ptr::null_mut(); 275],
        });
        // SAFETY: `list` has the header's layout and version; the driver
        // fills the pointers.
        if unsafe { create(&mut *list) } != NV_ENC_SUCCESS {
            return None;
        }
        Some(Self {
            _lib: lib,
            max_version,
            list,
        })
    }

    /// The highest API version the driver offers, (major, minor):
    /// "the 4 least significant bits … indicate the minor version"
    /// (nvEncodeAPI.h:4263-4265).
    fn max_api(&self) -> (u32, u32) {
        (self.max_version >> 4, self.max_version & 0xF)
    }

    /// Slot `slot` of the function list as `T`.
    ///
    /// # Safety
    /// `T` must be that slot's type in the header.
    unsafe fn slot<T: Copy>(&self, slot: usize) -> Option<T> {
        let ptr = self.list.fns[slot];
        // SAFETY: a function pointer has the size of a data pointer on our
        // targets, and the caller promises the type.
        (!ptr.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&ptr) })
    }

    /// Opens a session on `ctx` and reads the limits of H.264, H.265 and
    /// AV1. `None` when the driver refuses API 12.1 or a session.
    fn caps(&self, ctx: &Context<'_>) -> Option<[Option<NvencCodecCaps>; 3]> {
        // SAFETY: each slot's type is its header typedef (nvEncodeAPI.h:
        // 4322, 4323, 4328, 4349, 4351).
        let (open, count, guids, get_caps, destroy) = unsafe {
            (
                self.slot::<OpenSession>(SLOT_OPEN_ENCODE_SESSION_EX)?,
                self.slot::<GetGuidCount>(SLOT_GET_ENCODE_GUID_COUNT)?,
                self.slot::<GetGuids>(SLOT_GET_ENCODE_GUIDS)?,
                self.slot::<GetCaps>(SLOT_GET_ENCODE_CAPS)?,
                self.slot::<DestroyEncoder>(SLOT_DESTROY_ENCODER)?,
            )
        };
        let mut params = OpenSessionParams {
            version: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER,
            device_type: NV_ENC_DEVICE_TYPE_CUDA,
            device: ctx.raw,
            reserved: std::ptr::null_mut(),
            api_version: NVENCAPI_VERSION,
            reserved1: [0; 253],
            reserved2: [std::ptr::null_mut(); 64],
        };
        let mut encoder = std::ptr::null_mut();
        // SAFETY: `params` has the header's layout, `ctx` is a live CUDA
        // context, `encoder` a valid out pointer.
        if unsafe { open(&mut params, &mut encoder) } != NV_ENC_SUCCESS || encoder.is_null() {
            return None;
        }
        let session = Session { encoder, destroy };
        let mut n = 0;
        // SAFETY: a live session and a valid out pointer.
        if unsafe { count(session.encoder, &mut n) } != NV_ENC_SUCCESS {
            return None;
        }
        let mut list = vec![H264_GUID; n as usize];
        let mut got = 0;
        // SAFETY: `list` holds `n` GUIDs.
        if unsafe { guids(session.encoder, list.as_mut_ptr(), n, &mut got) } != NV_ENC_SUCCESS {
            return None;
        }
        list.truncate(got as usize);
        let query = |guid: Guid, what: u32| {
            let mut param = CapsParam {
                version: NV_ENC_CAPS_PARAM_VER,
                caps_to_query: what,
                reserved: [0; 62],
            };
            let mut value = 0;
            // SAFETY: a live session, `param` with the header's layout and
            // a valid out pointer.
            let r = unsafe { get_caps(session.encoder, guid, &mut param, &mut value) };
            (r == NV_ENC_SUCCESS).then(|| u32::try_from(value).unwrap_or(0))
        };
        let codec = |guid: Guid| {
            list.contains(&guid).then(|| NvencCodecCaps {
                max_width: query(guid, NV_ENC_CAPS_WIDTH_MAX).unwrap_or(0),
                max_height: query(guid, NV_ENC_CAPS_HEIGHT_MAX).unwrap_or(0),
                max_mb_per_sec: query(guid, NV_ENC_CAPS_MB_PER_SEC_MAX).unwrap_or(0),
                max_level: query(guid, NV_ENC_CAPS_LEVEL_MAX).unwrap_or(0),
                max_temporal_layers: query(guid, NV_ENC_CAPS_NUM_MAX_TEMPORAL_LAYERS).unwrap_or(0),
                dynamic_bitrate: query(guid, NV_ENC_CAPS_SUPPORT_DYN_BITRATE_CHANGE)
                    .is_some_and(|v| v != 0),
                engines: query(guid, NV_ENC_CAPS_NUM_ENCODER_ENGINES).unwrap_or(0),
            })
        };
        Some([codec(H264_GUID), codec(HEVC_GUID), codec(AV1_GUID)])
    }

    /// How many sessions can be open at once on `ctx`, up to `limit`
    /// (GeForce drivers cap it); a measurement for Task 3.
    fn session_limit(&self, ctx: &Context<'_>, limit: usize) -> Option<usize> {
        // SAFETY: as in `caps`.
        let (open, destroy) = unsafe {
            (
                self.slot::<OpenSession>(SLOT_OPEN_ENCODE_SESSION_EX)?,
                self.slot::<DestroyEncoder>(SLOT_DESTROY_ENCODER)?,
            )
        };
        let mut sessions = Vec::new();
        while sessions.len() < limit {
            let mut params = OpenSessionParams {
                version: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER,
                device_type: NV_ENC_DEVICE_TYPE_CUDA,
                device: ctx.raw,
                reserved: std::ptr::null_mut(),
                api_version: NVENCAPI_VERSION,
                reserved1: [0; 253],
                reserved2: [std::ptr::null_mut(); 64],
            };
            let mut encoder = std::ptr::null_mut();
            // SAFETY: as in `caps`.
            if unsafe { open(&mut params, &mut encoder) } != NV_ENC_SUCCESS || encoder.is_null() {
                break;
            }
            sessions.push(Session { encoder, destroy });
        }
        Some(sessions.len())
    }
}

/// An open NVENC session, destroyed on drop.
struct Session {
    encoder: *mut c_void,
    destroy: DestroyEncoder,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: opened by `nvEncOpenEncodeSessionEx` and destroyed once.
        unsafe { (self.destroy)(self.encoder) };
    }
}

// --- NVDEC (dynlink_cuviddec.h) -------------------------------------------

const CUDA_VIDEO_CODEC_H264: c_uint = 4; // dynlink_cuviddec.h, cudaVideoCodec_H264
const CUDA_VIDEO_CODEC_AV1: c_uint = 11; // cudaVideoCodec_AV1
const CUDA_VIDEO_CHROMA_420: c_uint = 1; // dynlink_cuviddec.h:122

/// CUVIDDECODECAPS (dynlink_cuviddec.h:159-181).
#[repr(C)]
#[derive(Default)]
struct DecodeCaps {
    codec: c_uint,
    chroma: c_uint,
    bit_depth_minus8: c_uint,
    reserved1: [c_uint; 3],
    is_supported: u8,
    num_nvdecs: u8,
    output_format_mask: u16,
    max_width: c_uint,
    max_height: c_uint,
    max_mb_count: c_uint,
    min_width: u16,
    min_height: u16,
    is_histogram_supported: u8,
    counter_bit_depth: u8,
    max_histogram_bins: u16,
    reserved3: [c_uint; 10],
}

type GetDecoderCaps = unsafe extern "system" fn(*mut DecodeCaps) -> CuResult;

struct Nvdec {
    _lib: Library,
    get_caps: GetDecoderCaps,
}

impl Nvdec {
    fn load() -> Option<Self> {
        let (lib, get_caps) = open(names::NVCUVID, |lib| {
            // SAFETY: dynlink_cuviddec.h:1060 (name from dynlink_loader.h:452).
            unsafe { symbol::<GetDecoderCaps>(lib, b"cuvidGetDecoderCaps\0") }
        })?;
        Some(Self {
            _lib: lib,
            get_caps,
        })
    }

    /// 8-bit 4:2:0 decode limits for `codec`, with a context current.
    fn caps(&self, _current: &Context<'_>, codec: c_uint) -> Option<NvdecCodecCaps> {
        let mut caps = DecodeCaps {
            codec,
            chroma: CUDA_VIDEO_CHROMA_420,
            ..DecodeCaps::default()
        };
        // SAFETY: `caps` has the header's layout; a context is current
        // (cuCtxCreate made it so), as the call needs.
        let r = unsafe { (self.get_caps)(&mut caps) };
        (r == CUDA_SUCCESS && caps.is_supported != 0).then_some(NvdecCodecCaps {
            max_width: caps.max_width,
            max_height: caps.max_height,
            engines: u32::from(caps.num_nvdecs),
        })
    }
}

// --- Probing ----------------------------------------------------------------

/// Fills `nvidia` for the NVIDIA GPUs among `gpus`.
pub(super) fn probe(gpus: &mut [Gpu]) {
    if !gpus.iter().any(|g| g.vendor == Vendor::Nvidia) {
        return;
    }
    let Some(cuda) = Cuda::load() else {
        return;
    };
    // SAFETY: flags must be 0.
    if unsafe { (cuda.f.init)(0) } != CUDA_SUCCESS {
        return;
    }
    let mut driver = 0;
    let mut count = 0;
    // SAFETY: valid out pointers.
    unsafe {
        (cuda.f.driver_get_version)(&mut driver);
        if (cuda.f.device_get_count)(&mut count) != CUDA_SUCCESS {
            return;
        }
    }
    let nvenc = Nvenc::load();
    let nvdec = Nvdec::load();
    for ordinal in 0..count {
        let mut device = 0;
        let mut uuid = CuUuid { bytes: [0; 16] };
        // SAFETY: valid out pointers; `ordinal` is below the count.
        let found = unsafe {
            (cuda.f.device_get)(&mut device, ordinal) == CUDA_SUCCESS
                && (cuda.f.device_get_uuid)(&mut uuid, device) == CUDA_SUCCESS
        };
        let Some(gpu) = found
            .then(|| gpus.iter_mut().find(|g| g.uuid == uuid.bytes))
            .flatten()
        else {
            continue;
        };
        let mut info = Nvidia {
            compute_capability: (
                cuda.attribute(CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR, device)
                    .unwrap_or(0),
                cuda.attribute(CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR, device)
                    .unwrap_or(0),
            ),
            cuda_driver_version: u32::try_from(driver).unwrap_or(0),
            nvenc_api: nvenc.as_ref().map(Nvenc::max_api),
            ..Nvidia::default()
        };
        if let Some(ctx) = cuda.context(device) {
            if let Some([h264, h265, av1]) = nvenc.as_ref().and_then(|e| e.caps(&ctx)) {
                info.nvenc_h264 = h264;
                info.nvenc_h265 = h265;
                info.nvenc_av1 = av1;
            }
            if let Some(d) = &nvdec {
                info.nvdec_h264 = d.caps(&ctx, CUDA_VIDEO_CODEC_H264);
                info.nvdec_av1 = d.caps(&ctx, CUDA_VIDEO_CODEC_AV1);
            }
            #[cfg(target_os = "linux")]
            if gpu.vulkan.external_memory_fd {
                info.vulkan_interop = Some(interop::check(&cuda, &ctx, gpu.uuid));
            }
        }
        gpu.nvidia = Some(info);
    }
}

/// How many NVENC sessions the first NVIDIA GPU with NVENC opens at once,
/// trying up to `limit` (`None` without one).
pub fn nvenc_session_limit(limit: usize) -> Option<usize> {
    let cuda = Cuda::load()?;
    let nvenc = Nvenc::load()?;
    // SAFETY: as in `probe`.
    unsafe {
        if (cuda.f.init)(0) != CUDA_SUCCESS {
            return None;
        }
    }
    let mut device = 0;
    // SAFETY: a valid out pointer; ordinal 0.
    if unsafe { (cuda.f.device_get)(&mut device, 0) } != CUDA_SUCCESS {
        return None;
    }
    let ctx = cuda.context(device)?;
    nvenc.session_limit(&ctx, limit)
}

/// Whether Vulkan memory exported as an opaque file descriptor imports
/// into CUDA and holds the same bytes on both sides: the path frames take
/// from our Vulkan compute shaders to NVENC (plan Q6).
#[cfg(target_os = "linux")]
mod interop {
    use ash::vk;

    use super::{
        CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD, CUDA_SUCCESS, Context, Cuda, ExternalHandle,
        ExternalMemoryBufferDesc, ExternalMemoryHandleDesc,
    };

    const SIZE: u64 = 1 << 20;

    pub(super) fn check(cuda: &Cuda, _ctx: &Context<'_>, uuid: [u8; 16]) -> bool {
        vulkan_side(uuid, |memory, size, fd| cuda_side(cuda, memory, size, fd)).unwrap_or(false)
    }

    /// Writes a pattern through CUDA into the imported memory, then reads
    /// it back on both sides.
    fn cuda_side(cuda: &Cuda, mapped: *mut u8, size: u64, fd: i32) -> bool {
        let (Some(import), Some(destroy), Some(map)) = (
            cuda.f.import_external_memory,
            cuda.f.destroy_external_memory,
            cuda.f.external_memory_get_mapped_buffer,
        ) else {
            close(fd);
            return false;
        };
        let desc = ExternalMemoryHandleDesc {
            kind: CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD,
            handle: ExternalHandle { fd },
            size,
            flags: 0,
            reserved: [0; 16],
        };
        let mut external = std::ptr::null_mut();
        // SAFETY: `desc` has the header's layout and a valid fd; on success
        // CUDA owns the fd.
        if unsafe { import(&mut external, &desc) } != CUDA_SUCCESS {
            close(fd);
            return false;
        }
        let buffer = ExternalMemoryBufferDesc {
            offset: 0,
            size,
            flags: 0,
            reserved: [0; 16],
        };
        let mut device_ptr = 0;
        // SAFETY: `external` was imported above; `buffer` has the header's
        // layout and stays inside the allocation.
        let ok = if unsafe { map(&mut device_ptr, external, &buffer) } == CUDA_SUCCESS {
            let pattern: Vec<u8> = (0..4096u32).map(|i| (i * 7 + 3) as u8).collect();
            let mut back = vec![0u8; pattern.len()];
            // SAFETY: both copies stay inside the mapped buffer (4096 bytes
            // of 1 MiB) and the host buffers have that length; `mapped` is
            // Vulkan's host mapping of the same memory, alive for this
            // closure, and the copies are synchronous.
            let same = unsafe {
                (cuda.f.memcpy_htod)(device_ptr, pattern.as_ptr().cast(), pattern.len())
                    == CUDA_SUCCESS
                    && (cuda.f.memcpy_dtoh)(back.as_mut_ptr().cast(), device_ptr, back.len())
                        == CUDA_SUCCESS
                    && back == pattern
                    && std::slice::from_raw_parts(mapped, pattern.len()) == pattern.as_slice()
            };
            // SAFETY: mapped above, freed once.
            unsafe { (cuda.f.mem_free)(device_ptr) };
            same
        } else {
            false
        };
        // SAFETY: imported above, destroyed once.
        unsafe { destroy(external) };
        ok
    }

    fn close(fd: i32) {
        use std::os::fd::{FromRawFd, OwnedFd};
        // SAFETY: Vulkan handed us this fd and nothing else owns it.
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
    }

    /// Makes a Vulkan device on the GPU with `uuid`, a 1 MiB host-visible,
    /// exportable buffer, and hands its host mapping, size and fd to
    /// `test`. Everything is released before returning.
    fn vulkan_side(uuid: [u8; 16], test: impl FnOnce(*mut u8, u64, i32) -> bool) -> Option<bool> {
        // SAFETY: as in `vulkan::probe`.
        let entry = unsafe { ash::Entry::load() }.ok()?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
        // SAFETY: `app` lives until the call returns.
        let instance = unsafe {
            entry.create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
        }
        .ok()?;
        let result = with_instance(&instance, uuid, test);
        // SAFETY: nothing made from the instance is alive any more.
        unsafe { instance.destroy_instance(None) };
        result
    }

    fn with_instance(
        instance: &ash::Instance,
        uuid: [u8; 16],
        test: impl FnOnce(*mut u8, u64, i32) -> bool,
    ) -> Option<bool> {
        // SAFETY: a live instance.
        let physical = unsafe { instance.enumerate_physical_devices() }
            .ok()?
            .into_iter()
            .find(|pd| {
                let mut id = vk::PhysicalDeviceIDProperties::default();
                let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
                // SAFETY: `pd` comes from this instance; `id` lives until
                // the call returns.
                unsafe { instance.get_physical_device_properties2(*pd, &mut props) };
                id.device_uuid == uuid
            })?;
        let priorities = [1.0];
        let queue = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(0)
            .queue_priorities(&priorities);
        let extensions = [vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr()];
        let info = vk::DeviceCreateInfo::default()
            .queue_create_infos(std::slice::from_ref(&queue))
            .enabled_extension_names(&extensions);
        // SAFETY: `info` and what it points to live until the call
        // returns; the extension was found on this device by the probe.
        let device = unsafe { instance.create_device(physical, &info, None) }.ok()?;
        let result = with_device(instance, physical, &device, test);
        // SAFETY: everything made from the device was released in
        // `with_device`.
        unsafe { device.destroy_device(None) };
        result
    }

    fn with_device(
        instance: &ash::Instance,
        physical: vk::PhysicalDevice,
        device: &ash::Device,
        test: impl FnOnce(*mut u8, u64, i32) -> bool,
    ) -> Option<bool> {
        let mut external = vk::ExternalMemoryBufferCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let info = vk::BufferCreateInfo::default()
            .size(SIZE)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST)
            .push_next(&mut external);
        // SAFETY: `info` lives until the call returns.
        let buffer = unsafe { device.create_buffer(&info, None) }.ok()?;
        // SAFETY: a live buffer of this device.
        let needs = unsafe { device.get_buffer_memory_requirements(buffer) };
        // SAFETY: `physical` is the device's GPU.
        let types = unsafe { instance.get_physical_device_memory_properties(physical) };
        let wanted = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let index = (0..types.memory_type_count).find(|i| {
            needs.memory_type_bits & (1 << i) != 0
                && types.memory_types[*i as usize]
                    .property_flags
                    .contains(wanted)
        });
        let result = index.and_then(|index| {
            let mut export = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let alloc = vk::MemoryAllocateInfo::default()
                .allocation_size(needs.size)
                .memory_type_index(index)
                .push_next(&mut export);
            // SAFETY: `alloc` lives until the call returns.
            let memory = unsafe { device.allocate_memory(&alloc, None) }.ok()?;
            let ok = (|| {
                // SAFETY: both belong to this device; offset 0.
                unsafe { device.bind_buffer_memory(buffer, memory, 0) }.ok()?;
                // SAFETY: host-visible memory of this device, mapped once.
                let mapped = unsafe {
                    device.map_memory(memory, 0, needs.size, vk::MemoryMapFlags::empty())
                }
                .ok()?
                .cast::<u8>();
                let fds = ash::khr::external_memory_fd::Device::new(instance, device);
                let get = vk::MemoryGetFdInfoKHR::default()
                    .memory(memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
                // SAFETY: memory allocated exportable as an opaque fd.
                let fd = unsafe { fds.get_memory_fd(&get) }.ok();
                let ok = fd.map(|fd| test(mapped, needs.size, fd));
                // SAFETY: mapped above.
                unsafe { device.unmap_memory(memory) };
                ok
            })();
            // SAFETY: allocated above; CUDA's import, if any, was destroyed
            // in `test`.
            unsafe { device.free_memory(memory, None) };
            ok
        });
        // SAFETY: created above; its memory is freed.
        unsafe { device.destroy_buffer(buffer, None) };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structures_have_the_headers_sizes() {
        // On 64-bit targets: 8 + 43 × 8 + 275 × 8.
        assert_eq!(std::mem::size_of::<FunctionList>(), 8 + 318 * 8);
        // 4 + 4 + 8 + 8 + 4 + 253 × 4 + 64 × 8.
        assert_eq!(std::mem::size_of::<OpenSessionParams>(), 1552);
        assert_eq!(std::mem::size_of::<CapsParam>(), 64 * 4);
        assert_eq!(std::mem::size_of::<DecodeCaps>(), 88);
        assert_eq!(std::mem::size_of::<Guid>(), 16);
        // type (4, padded to 8), the union (16), size (8), flags (4), 16 × 4.
        assert_eq!(std::mem::size_of::<ExternalMemoryHandleDesc>(), 104);
        assert_eq!(std::mem::size_of::<ExternalMemoryBufferDesc>(), 88);
    }

    #[test]
    fn struct_versions_follow_the_header_macro() {
        assert_eq!(NVENCAPI_VERSION, 0x0100_000C);
        assert_eq!(struct_version(1), 0x7101_000C);
        assert_eq!(NV_ENCODE_API_FUNCTION_LIST_VER, 0x7102_000C);
    }
}
