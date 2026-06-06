#![allow(clippy::missing_safety_doc)]

pub mod cli;
pub mod config;
pub mod core;
pub mod error;

#[cfg(any(target_os = "linux", target_os = "macos"))]
extern crate blas_src;

pub mod memory;
pub mod model;
pub mod tools;

// Compatibility shims for GLIBC 2.38 C23 string parsing functions to run
// precompiled ONNX Runtime on older GLIBC versions (e.g. Ubuntu 22.04 with GLIBC 2.35)
unsafe extern "C" {
    fn strtoll(
        nptr: *const std::ffi::c_char,
        endptr: *mut *mut std::ffi::c_char,
        base: std::ffi::c_int,
    ) -> std::ffi::c_longlong;
    fn strtoull(
        nptr: *const std::ffi::c_char,
        endptr: *mut *mut std::ffi::c_char,
        base: std::ffi::c_int,
    ) -> std::ffi::c_ulonglong;
    fn strtol(
        nptr: *const std::ffi::c_char,
        endptr: *mut *mut std::ffi::c_char,
        base: std::ffi::c_int,
    ) -> std::ffi::c_long;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtoll(
    nptr: *const std::ffi::c_char,
    endptr: *mut *mut std::ffi::c_char,
    base: std::ffi::c_int,
) -> std::ffi::c_longlong {
    unsafe { strtoll(nptr, endptr, base) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtoull(
    nptr: *const std::ffi::c_char,
    endptr: *mut *mut std::ffi::c_char,
    base: std::ffi::c_int,
) -> std::ffi::c_ulonglong {
    unsafe { strtoull(nptr, endptr, base) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtol(
    nptr: *const std::ffi::c_char,
    endptr: *mut *mut std::ffi::c_char,
    base: std::ffi::c_int,
) -> std::ffi::c_long {
    unsafe { strtol(nptr, endptr, base) }
}
