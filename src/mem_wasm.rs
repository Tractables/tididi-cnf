//! `mem` on WebAssembly, which has neither jemalloc nor an address-space
//! limit: the module's linear memory grows on demand, under the target's own
//! allocator, up to the maximum the program was linked with. The native
//! module, `mem.rs`, says what each function here does there. Each keeps its
//! signature and answers as a process with no limit set, so the library's
//! memory hooks and the recovery cascade compile unchanged and stay inert, as
//! they do natively without a limit.

/// Nothing to check: there is no limit to approach.
pub(crate) fn preflight_alloc_decay_check(_request_bytes: u64) {}

/// There are no arenas to purge: always `ENOSYS`.
pub(crate) fn purge_all_arenas() -> Result<(), libc::c_int> {
    Err(libc::ENOSYS)
}

/// There is no page reclamation to switch: always `ENOSYS`.
pub(crate) fn reclaim_eagerly() -> Result<(), libc::c_int> {
    Err(libc::ENOSYS)
}

/// The size of the module's linear memory, the address space it holds.
pub(crate) fn mapped_bytes() -> u64 {
    (std::arch::wasm32::memory_size::<0>() as u64) * 65536
}

/// The heap counters, with [`mapped_bytes`] as `mapped`. The allocator
/// reports neither of the others, so both read 0.
#[derive(Clone, Copy)]
pub(crate) struct HeapStats {
    pub(crate) allocated: u64,
    pub(crate) mapped: u64,
    pub(crate) retained: u64,
}

impl HeapStats {
    pub(crate) fn read() -> Self {
        HeapStats { allocated: 0, mapped: mapped_bytes(), retained: 0 }
    }
}

/// Always `None`, which every caller reads as no cap.
pub(crate) fn rlimit_as_bytes() -> Option<u64> {
    None
}

/// A run's memory limit: there is none to hold.
pub(crate) struct AddressSpaceLimit;

/// Accept a memory limit of 0, and refuse any other: there is no
/// address-space limit to lower, so the run would ignore it.
pub(crate) fn limit_address_space(mb: u64) -> Result<AddressSpaceLimit, crate::Error> {
    if mb == 0 {
        return Ok(AddressSpaceLimit);
    }
    Err(crate::Error::Usage(format!(
        "a memory limit of {mb} MiB needs a native build: WebAssembly has no \
         address-space limit to lower. Leave it at 0."
    )))
}
