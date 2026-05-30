use std::ffi::c_void;

use crate::builder::RunContext;

/// RED-style job hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum JobHint {
    None,
    Trivial,
    Large,
    PhysX,
    AudioEvent,
}

/// RED-style job debug flags.
///
/// Kept as raw bits because RED exposes this as a flag byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobDebugFlags;

impl JobDebugFlags {
    pub const ALLOW_FRAME_ALLOCATOR: u8 = 1 << 0;
}

pub type JobFunc = unsafe fn(*mut c_void, &RunContext);
pub type ParallelForJobFunc = unsafe fn(*mut c_void, *mut c_void, u32, u32, &RunContext);
pub type SharedDataInitCallback = unsafe fn(u32, *mut c_void) -> *mut c_void;
pub type EpilogueFunc = unsafe fn(*mut c_void, *mut c_void, u32, &RunContext);

/// Mirror of RED `job::JobDecl`.
///
/// The raw `job_data` pointer is intentionally non-owning, matching RED. The
/// caller must ensure it remains valid until the queued job runs.
#[derive(Clone, Copy)]
pub struct JobDecl {
    pub job_func: Option<JobFunc>,
    pub job_data: *mut c_void,
    pub instrumentation_object: Option<&'static str>,
    pub hint: JobHint,
    pub debug_flags: u8,
}

unsafe impl Send for JobDecl {}
unsafe impl Sync for JobDecl {}

impl Default for JobDecl {
    fn default() -> Self {
        Self {
            job_func: None,
            job_data: std::ptr::null_mut(),
            instrumentation_object: None,
            hint: JobHint::None,
            debug_flags: 0,
        }
    }
}

impl JobDecl {
    /// Run this raw RED-style job declaration immediately.
    ///
    /// # Safety
    ///
    /// `job_data` must be valid for `job_func` for the entire duration of the
    /// call, and the pointed-to data must satisfy whatever thread-safety and
    /// aliasing requirements `job_func` relies on.
    pub unsafe fn run(self, run_context: &RunContext) {
        let job_func = self
            .job_func
            .expect("[leet_jobs] JobDecl::job_func must be set before dispatch");
        // SAFETY: this mirrors RED's raw function-pointer + void* contract.
        unsafe {
            job_func(self.job_data, run_context);
        }
    }
}

/// Owned Rust-side wrapper around RED's raw `JobDecl`.
///
/// RED stores non-owning pointers in `JobDecl`. LEET keeps that shape for the
/// public mirror, but closure-backed jobs need one extra cleanup hook so queued
/// closures are dropped if the dispatcher shuts down before they run.
pub(crate) struct OwnedJobDecl {
    job_decl: JobDecl,
    drop_job_data: Option<unsafe fn(*mut c_void)>,
}

unsafe impl Send for OwnedJobDecl {}

impl OwnedJobDecl {
    pub(crate) fn borrowed(job_decl: JobDecl) -> Self {
        Self {
            job_decl,
            drop_job_data: None,
        }
    }

    pub(crate) fn from_closure<F>(
        job: F,
        hint: JobHint,
        instrumentation_object: Option<&'static str>,
    ) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        let job_data = Box::into_raw(Box::new(ClosureJobData {
            job: Some(Box::new(job)),
        })) as *mut c_void;

        Self {
            job_decl: JobDecl {
                job_func: Some(run_closure_job),
                job_data,
                instrumentation_object,
                hint,
                debug_flags: 0,
            },
            drop_job_data: Some(drop_closure_job_data),
        }
    }

    pub(crate) fn job_decl(&self) -> JobDecl {
        self.job_decl
    }

    pub(crate) fn hint(&self) -> JobHint {
        self.job_decl.hint
    }

    pub(crate) fn run(mut self, run_context: &RunContext) {
        let job_decl = self.job_decl;
        self.drop_job_data = None;
        // SAFETY: callers can only create closure-backed `OwnedJobDecl`s
        // through `from_closure`, or raw borrowed declarations through an
        // unsafe submission API that requires the caller to uphold the
        // non-owning pointer contract.
        unsafe {
            job_decl.run(run_context);
        }
    }
}

impl Drop for OwnedJobDecl {
    fn drop(&mut self) {
        if let Some(drop_job_data) = self.drop_job_data.take() {
            if !self.job_decl.job_data.is_null() {
                // SAFETY: `drop_job_data` matches the allocation used to create
                // this owned job declaration.
                unsafe {
                    drop_job_data(self.job_decl.job_data);
                }
            }
        }
    }
}

struct ClosureJobData {
    job: Option<Box<dyn FnOnce() + Send>>,
}

unsafe fn run_closure_job(job_data: *mut c_void, _run_context: &RunContext) {
    // SAFETY: closure jobs are created by `OwnedJobDecl::from_closure`, which
    // stores exactly this allocation in `JobDecl::job_data`.
    let mut job_data = unsafe { Box::from_raw(job_data as *mut ClosureJobData) };
    let job = job_data
        .job
        .take()
        .expect("[leet_jobs] closure job already consumed");
    job();
}

unsafe fn drop_closure_job_data(job_data: *mut c_void) {
    // SAFETY: closure jobs are created by `OwnedJobDecl::from_closure`, and
    // this path is only used when the job was never executed.
    unsafe {
        drop(Box::from_raw(job_data as *mut ClosureJobData));
    }
}

/// Mirror of RED `job::JobDeclParallelFor`.
///
/// `shared_data` and `elements` are non-owning raw pointers, matching RED.
#[derive(Clone, Copy)]
pub struct JobDeclParallelFor {
    pub job_func: Option<ParallelForJobFunc>,
    pub init_shared_data_callback: Option<SharedDataInitCallback>,
    pub epilogue_func: Option<EpilogueFunc>,
    pub shared_data: *mut c_void,
    pub elements: *mut c_void,
    pub num_elements: u32,
    pub max_batch_size: u32,
    pub instrumentation_object: Option<&'static str>,
    pub debug_flags: u8,
}

unsafe impl Send for JobDeclParallelFor {}
unsafe impl Sync for JobDeclParallelFor {}

impl Default for JobDeclParallelFor {
    fn default() -> Self {
        Self {
            job_func: None,
            init_shared_data_callback: None,
            epilogue_func: None,
            shared_data: std::ptr::null_mut(),
            elements: std::ptr::null_mut(),
            num_elements: 0,
            max_batch_size: 0,
            instrumentation_object: None,
            debug_flags: 0,
        }
    }
}
