//! Compiler requests and deterministic failure categories.

use crate::{code_cache::CompiledArtifact, runtime::CompileRequest};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct CompileControl {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
    max_ir_bytes: usize,
}

impl CompileControl {
    pub fn new(cancelled: Arc<AtomicBool>, budget: Duration) -> Self {
        Self::with_ir_limit(cancelled, budget, usize::MAX)
    }
    pub fn with_ir_limit(
        cancelled: Arc<AtomicBool>,
        budget: Duration,
        max_ir_bytes: usize,
    ) -> Self {
        Self {
            cancelled,
            deadline: Instant::now() + budget,
            max_ir_bytes,
        }
    }
    pub fn check_ir_bytes(&self, bytes: usize) -> Result<(), CompileFailure> {
        self.check()?;
        if bytes > self.max_ir_bytes {
            Err(CompileFailure::ResourceLimit)
        } else {
            Ok(())
        }
    }
    pub fn check(&self) -> Result<(), CompileFailure> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(CompileFailure::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(CompileFailure::TimedOut)
        } else {
            Ok(())
        }
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
pub mod baseline;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
mod call_cleanup;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
mod helpers;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
pub mod native_call;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
pub mod optimized;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
mod refcount;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
mod tagged_call_link;

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
struct DirectCalleeIdentity {
    object: u64,
    bytecode: u64,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn emit_guarded_direct_callee_identity(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    tag: cranelift_codegen::ir::Value,
    payload: cranelift_codegen::ir::Value,
    callee_identity: DirectCalleeIdentity,
    pointer_type: cranelift_codegen::ir::Type,
    matched: cranelift_codegen::ir::Block,
    miss: cranelift_codegen::ir::Block,
) {
    use cranelift_codegen::ir::{condcodes::IntCC, InstBuilder, MemFlags};

    let identity = builder.create_block();
    let bytecode = builder.create_block();
    let object_tag = builder.ins().icmp_imm(
        IntCC::Equal,
        tag,
        i64::from(rquickjs_core::qjs::JS_TAG_OBJECT),
    );
    builder.ins().brif(object_tag, identity, &[], miss, &[]);

    builder.switch_to_block(identity);
    let identity_matches =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, payload, callee_identity.object as i64);
    builder
        .ins()
        .brif(identity_matches, bytecode, &[], miss, &[]);

    // JSObject::u.func.function_bytecode is at byte offset 48 on the
    // fingerprinted 64-bit QuickJS layout. The payload is dereferenced only
    // after the tag and rooted object identity have both matched.
    builder.switch_to_block(bytecode);
    let function_bytecode = builder
        .ins()
        .load(pointer_type, MemFlags::new(), payload, 48);
    let bytecode_matches = builder.ins().icmp_imm(
        IntCC::Equal,
        function_bytecode,
        callee_identity.bytecode as i64,
    );
    builder
        .ins()
        .brif(bytecode_matches, matched, &[], miss, &[]);
}

#[cfg(any(test, feature = "test-support"))]
pub mod mock;

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[cfg(rquickjs_memory_sanitizer)]
unsafe extern "C" fn clear_msan_param_shadow() {
    unsafe extern "C" {
        #[thread_local]
        static mut __msan_param_tls: [usize; 100];
    }

    unsafe {
        let address = core::ptr::addr_of_mut!(__msan_param_tls).cast::<usize>();
        core::arch::asm!(
            "rep stosq",
            inout("rcx") 100usize => _,
            inout("rdi") address => _,
            inout("rax") 0usize => _,
            options(nostack),
        );
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[cfg(rquickjs_memory_sanitizer)]
unsafe extern "C" fn unpoison_jit_frame(frame: *mut rquickjs_core::qjs::JSJitExecFrame) {
    unsafe extern "C" {
        fn __msan_unpoison(address: *const core::ffi::c_void, size: usize);
    }

    if frame.is_null() {
        return;
    }
    unsafe {
        __msan_unpoison(
            frame.cast(),
            core::mem::size_of::<rquickjs_core::qjs::JSJitExecFrame>(),
        );
        let values_start = (*frame).arg_buf.cast::<u8>();
        let values_end = (*frame).stack_capacity.cast::<u8>();
        if !values_start.is_null() {
            if let Some(values_size) = (values_end as usize).checked_sub(values_start as usize) {
                if values_size <= 64 * 1024 * 1024 {
                    __msan_unpoison(values_start.cast(), values_size);
                }
            }
        }
        let locals_start = (*frame).var_buf.cast::<u8>();
        if !locals_start.is_null() {
            if let Some(locals_size) = (values_end as usize).checked_sub(locals_start as usize) {
                if locals_size <= 64 * 1024 * 1024 {
                    __msan_unpoison(locals_start.cast(), locals_size);
                }
            }
        }
        let start = (*frame).stack_base.cast::<u8>();
        let end = (*frame).stack_capacity.cast::<u8>();
        if !start.is_null() {
            if let Some(size) = (end as usize).checked_sub(start as usize) {
                __msan_unpoison(start.cast(), size);
            }
        }
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[cfg(rquickjs_memory_sanitizer)]
unsafe extern "C" fn unpoison_jit_bytes(start: *const core::ffi::c_void, size: usize) {
    unsafe extern "C" {
        fn __msan_unpoison(address: *const core::ffi::c_void, size: usize);
    }

    if !start.is_null() {
        unsafe { __msan_unpoison(start, size) };
    }
}

/// Marks bytes that generated code stored as initialized before an
/// instrumented callee reads them. MSan cannot observe Cranelift stores, so
/// stack slots passed by address otherwise keep stale shadow. No-op outside
/// MSan builds.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn emit_msan_unpoison(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    pointer_type: cranelift_codegen::ir::Type,
    address: cranelift_codegen::ir::Value,
    size: usize,
) {
    #[cfg(rquickjs_memory_sanitizer)]
    {
        use cranelift_codegen::ir::{AbiParam, InstBuilder, Signature};

        builder.set_srcloc(Default::default());
        let mut signature = Signature::new(builder.func.signature.call_conv);
        signature
            .params
            .extend([AbiParam::new(pointer_type), AbiParam::new(pointer_type)]);
        let signature = builder.import_signature(signature);
        let target = builder.ins().iconst(
            pointer_type,
            unpoison_jit_bytes as *const () as usize as i64,
        );
        let size = builder.ins().iconst(pointer_type, size as i64);
        builder
            .ins()
            .call_indirect(signature, target, &[address, size]);
    }

    #[cfg(not(rquickjs_memory_sanitizer))]
    let _ = (builder, pointer_type, address, size);
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn emit_external_call(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    signature: cranelift_codegen::ir::SigRef,
    target: cranelift_codegen::ir::Value,
    params: &[cranelift_codegen::ir::Value],
    pointer_type: cranelift_codegen::ir::Type,
    frame: Option<cranelift_codegen::ir::Value>,
    source_location: Option<cranelift_codegen::ir::SourceLoc>,
) -> cranelift_codegen::ir::Inst {
    use cranelift_codegen::ir::InstBuilder;
    #[cfg(rquickjs_memory_sanitizer)]
    use cranelift_codegen::ir::Signature;

    #[cfg(rquickjs_memory_sanitizer)]
    {
        builder.set_srcloc(Default::default());
        let msan_signature = Signature::new(builder.func.signature.call_conv);
        let msan_signature = builder.import_signature(msan_signature);
        let msan_target = builder.ins().iconst(
            pointer_type,
            clear_msan_param_shadow as *const () as usize as i64,
        );
        builder
            .ins()
            .call_indirect(msan_signature, msan_target, &[]);
        if let Some(frame) = frame {
            let mut frame_signature = Signature::new(builder.func.signature.call_conv);
            frame_signature
                .params
                .push(cranelift_codegen::ir::AbiParam::new(pointer_type));
            let frame_signature = builder.import_signature(frame_signature);
            let frame_target = builder.ins().iconst(
                pointer_type,
                unpoison_jit_frame as *const () as usize as i64,
            );
            builder
                .ins()
                .call_indirect(frame_signature, frame_target, &[frame]);
            builder
                .ins()
                .call_indirect(msan_signature, msan_target, &[]);
        }
    }

    #[cfg(not(rquickjs_memory_sanitizer))]
    let _ = (pointer_type, frame);

    if let Some(source_location) = source_location {
        builder.set_srcloc(source_location);
    }

    builder.ins().call_indirect(signature, target, params)
}

/// A worker-side compiler implementation.
pub trait Compiler: Send + Sync {
    fn compile(&self, request: CompileRequest) -> Result<CompiledArtifact, CompileFailure>;

    fn compile_controlled(
        &self,
        request: CompileRequest,
        control: &CompileControl,
    ) -> Result<CompiledArtifact, CompileFailure> {
        control.check()?;
        let result = self.compile(request);
        control.check()?;
        result
    }
}

/// Closed compiler failure categories used by the tiering policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileFailure {
    UnsupportedOpcode,
    Tier1Rejected(crate::bytecode::FallbackReason),
    ResourceLimit,
    TimedOut,
    Cancelled,
    CompilerPanicked,
    InvalidArtifact,
}
