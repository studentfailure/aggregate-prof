pub mod arch;

#[cfg(test)]
mod tests;

use arch::clock as clock;

use std::{sync::{OnceLock, atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering}}};

/// Defines a profiling scope with the given name. The scope will automatically end when the current block is exited, and the time spent in the scope will be recorded for the current thread.
#[macro_export]
macro_rules! scope 
{
  ($name: literal) => 
  {
    let _guard =
    {
      static SITE: $crate::CallSite = $crate::CallSite::new($name, file!(), line!());
      $crate::start(&SITE)
    };
  };
}

#[inline(always)]
const fn log2(v: u64) -> u32
{
  63 - (v | 1).leading_zeros()
}

/// Starts a profiling scope for the given call site. The scope will automatically end when the returned guard is dropped.
/// 
/// # Note
/// Should not be called directly, use the `scope!` macro instead.
#[inline(always)]
pub fn start<'a>(call_site: &'static CallSite) -> Guard<'a>
{
  Guard::new(call_site)
}

static CONFIG: OnceLock<ProfilerConfiguration> = OnceLock::new();

/// Initializes the profiler with the given configuration.
/// 
/// # Panics
/// Panics if the profiler has already been initialized.
pub fn init(profiler: ProfilerConfiguration)
{
  match try_init(profiler)
  {
    Ok(_) => {},
    Err(_) => panic!("profiler has already been initialized"),
  }
}

/// Initializes the profiler with the given configuration. This function must be called before any profiling is done, and it can only be called once.
/// 
/// # Errors
/// Returns the given configuration if the profiler has already been initialized.
pub fn try_init(profiler: ProfilerConfiguration) -> Result<(), ProfilerConfiguration>
{
  CONFIG.set(profiler).map_err(|_| profiler)
}

/// Converts ticks to nanoseconds.
pub fn ticks_to_nanos(ticks: u64) -> u64
{
  clock::ticks_to_nanos(ticks)
}

/// Profiler configuration
#[derive(Debug, Clone, Copy)]
pub struct ProfilerConfiguration
{
  thread_aggregate_config: ThreadAggregateConfig,
}

impl ProfilerConfiguration
{
  pub const fn new(thread_aggregate_config: ThreadAggregateConfig) -> Self
  {
    Self { thread_aggregate_config }
  }

  /// Sets the thread aggregate configuration for the profiler.
  pub fn with_thread_aggregate_config(mut self, config: ThreadAggregateConfig) -> Self
  {
    self.thread_aggregate_config = config;
    self
  }

  /// Returns the thread aggregate configuration for the profiler.
  pub const fn thread_aggregate_config(&self) -> ThreadAggregateConfig
  {
    self.thread_aggregate_config
  }
}

impl Default for ProfilerConfiguration
{
  fn default() -> Self
  {
    Self::new(ThreadAggregateConfig::default())
  }
}

struct AggregateStatBucket(Box<[AtomicU64]>);

impl AggregateStatBucket
{
  fn new(size: usize) -> Self
  {
    // SAFETY: all-zero bytes are a valid AtomicU64
    Self(unsafe { Box::new_zeroed_slice(size).assume_init() })
  }

  #[inline(always)]
  fn increment_plain(&self, index: usize, amount: u64)
  {
    let atomic = &self.0[index];
    atomic.store(atomic.load(Ordering::Relaxed) + amount, Ordering::Relaxed);
  }

  #[inline(always)]
  pub(crate) fn read(&self, index: usize) -> u64
  {
    self.0[index].load(Ordering::Acquire)
  }

  #[inline(always)]
  pub(crate) const fn len(&self) -> usize
  {
    self.0.len()
  }
}

struct AggregateStatBucketContainer<const LEN: usize>([AtomicPtr<AggregateStatBucket>; LEN]);

impl<const LEN: usize> AggregateStatBucketContainer<LEN>
{

  const FIRST_BUCKET_LEN_LOG2: u32 = 6;
  const FIRST_BUCKET_LEN: usize = 1 << Self::FIRST_BUCKET_LEN_LOG2;

  pub(crate) fn new() -> Self
  {
    Self([const { AtomicPtr::new(std::ptr::null_mut()) }; LEN])
  }  

  #[inline(always)]
  pub(crate) fn bucket(&self, index: usize) -> Option<&AggregateStatBucket>
  {
    unsafe {
      let bucket_ptr = self.0[Self::bucket_index(index)].load(Ordering::Acquire);
      if bucket_ptr.is_null() {
        None
      } 
      else {
        Some(&*bucket_ptr)
      }
    }
  }

  #[cold]
  pub(crate) fn allocate_bucket(&self, index: usize) -> &AggregateStatBucket
  {
    let bucket_index = Self::bucket_index(index);
    let new_bucket = Box::into_raw(Box::new(AggregateStatBucket::new(Self::bucket_size(bucket_index))));
    self.0[bucket_index].store(new_bucket, Ordering::Release);
    unsafe { &*new_bucket }
  }
  
  #[inline(always)]
  fn increment_plain(&self, index: usize, amount: u64)
  {
    let bucket = match self.bucket(index)
    {
      Some(bucket_opt) => bucket_opt,
      None => self.allocate_bucket(index),
    };
    bucket.increment_plain(Self::slot_in_bucket(index, bucket.len()), amount);
  }

  #[inline(always)]
  fn get(&self, index: usize) -> Option<u64>
  {
    self.bucket(index).map(|bucket| bucket.read(Self::slot_in_bucket(index, bucket.len())))
  }

  #[inline(always)]
  fn set(&self, index: usize, value: u64)
  {
    let bucket = match self.bucket(index)
    {
      Some(bucket_opt) => bucket_opt,
      None => self.allocate_bucket(index),
    };
    bucket.0[Self::slot_in_bucket(index, bucket.len())].store(value, Ordering::Release);
  }

  #[inline(always)]
  const fn bucket_index(slot_index: usize) -> usize
  {
    let shifted_index = slot_index + Self::FIRST_BUCKET_LEN;
    (usize::BITS - 1 - shifted_index.leading_zeros() - Self::FIRST_BUCKET_LEN_LOG2) as usize
  }

  #[inline(always)]
  const fn bucket_size(bucket_index: usize) -> usize
  {
    Self::FIRST_BUCKET_LEN << bucket_index
  }

  #[inline(always)]
  const fn slot_in_bucket(slot_index: usize, bucket_size: usize) -> usize
  {
    slot_index + Self::FIRST_BUCKET_LEN - bucket_size
  }
}

pub struct CallSiteAggregateStatSnapshot
{
  pub count: u64,
  pub total_time: u64,
  pub max_time: u64,
  pub bins: Box<[u64]>,
}

const BUCKETS: usize = 32;

#[derive(Debug, Clone, Copy)]
pub struct ThreadAggregateConfig
{
  time_bin_count: usize,
  time_bin_log2_offset: u32
}

impl ThreadAggregateConfig
{
  /// Bin 0 holds samples under 2^(offset + 1) ticks, and each bin after it doubles.
  /// The top bin is open-ended, starting at 2^(offset + count - 1) ticks.
  ///
  /// The defaults (16, 8) cover about 190 ns to 3.1 ms at 2.688 ticks/ns.
  ///
  /// # Panics
  /// Panics if `time_bin_count` is not in `1..=64`.
  pub const fn new(time_bin_count: usize, time_bin_log2_offset: u32) -> Self
  {
    assert!(time_bin_count >= 1 && time_bin_count <= 64, "time_bin_count must be in 1..=64");
    Self { time_bin_count, time_bin_log2_offset }
  }

  /// Returns the number of time bins used for aggregating call site statistics.
  pub const fn time_bin_count(&self) -> usize
  {
    self.time_bin_count
  }

  /// Returns the log2 offset used for determining the time bin for a given sample.
  pub const fn time_bin_log2_offset(&self) -> u32
  {
    self.time_bin_log2_offset
  }
}

impl Default for ThreadAggregateConfig
{
  fn default() -> Self
  {
    Self::new(16, 8)
  }
}

pub struct ThreadAggregate
{
  config: ThreadAggregateConfig,
  thread_name: String,
  bins: AggregateStatBucketContainer<BUCKETS>,
  total_time: AggregateStatBucketContainer<BUCKETS>,
  max_time: AggregateStatBucketContainer<BUCKETS>,
  free: AtomicBool,
  next: AtomicPtr<ThreadAggregate>,
}

impl ThreadAggregate
{
  fn new_pre_occupied(config: ProfilerConfiguration, thread_name: &str) -> Self
  {
    Self 
    {
      config: config.thread_aggregate_config,
      thread_name: thread_name.to_string(),
      bins: AggregateStatBucketContainer::new(),
      total_time: AggregateStatBucketContainer::new(),
      max_time: AggregateStatBucketContainer::new(),
      free: AtomicBool::new(false),
      next: AtomicPtr::new(std::ptr::null_mut())
    }
  }

  /// Returns a snapshot of the aggregate statistics for the given call site ID.
  pub fn snapshot(&self, call_site_id: u32) -> CallSiteAggregateStatSnapshot
  {
    let time_bins = self.config.time_bin_count;
    let bins: Box<[u64]> = (0..time_bins).map(|bin|
    {
      self.bins.get(call_site_id as usize * time_bins + bin).unwrap_or(0)
    })
    .collect();
    let count = bins.iter().sum();
    let total_time = self.total_time
      .bucket(call_site_id as usize)
      .map_or(0, |bucket| bucket.read(AggregateStatBucketContainer::<BUCKETS>::slot_in_bucket(call_site_id as usize, bucket.len())));
    let max_time = self.max_time
      .get(call_site_id as usize)
      .unwrap_or(0);
    CallSiteAggregateStatSnapshot { count, total_time, max_time, bins }
  }

  /// Returns the configuration used for this thread aggregate.
  pub const fn config(&self) -> ThreadAggregateConfig
  {
    self.config
  }

  #[inline(always)]
  fn record(&self, call_site_id: u32, time: u64)
  {
    self.increment_bin(call_site_id, time);
    self.increment_total_time(call_site_id, time);
    let max_time = self.max_time
      .get(call_site_id as usize)
      .unwrap_or(0);
    if time > max_time
    {
      self.max_time.set(call_site_id as usize, time);
    }
  }
  
  #[inline(always)]
  fn increment_bin(&self, call_site_id: u32, time: u64)
  {
    let time_bin_count = self.config.time_bin_count;
    let time_bin_log2_offset = self.config.time_bin_log2_offset;
    let bin = (log2(time).saturating_sub(time_bin_log2_offset) as usize).min(time_bin_count - 1);
    self.bins.increment_plain(call_site_id as usize * time_bin_count + bin, 1);
  }

  #[inline(always)]
  fn increment_total_time(&self, call_site_id: u32, time: u64)
  {
    self.total_time.increment_plain(call_site_id as usize, time);
  }

  pub(crate) fn mark_free(&self)
  {
    self.free.store(true, Ordering::Release);
  }

  pub(crate) fn cas_free(&self) -> bool
  {
    self.free.compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire).is_ok()
  }
}

/// Iterates over all registered thread aggregates, calling the closure for each one.
pub fn for_each_thread_aggregate<F>(mut f: F)
  where
    F: FnMut(&'static ThreadAggregate)
{
  let mut current = THREAD_AGGREGATE_LL_HEAD.load(Ordering::Acquire);
  while !current.is_null()
  {
    let node = unsafe { &*current };
    f(node);
    current = node.next.load(Ordering::Acquire);
  }
}

/// Iterates over all registered call sites, calling the closure for each one.
pub fn for_each_call_site<F>(mut f: F)
  where
    F: FnMut(&'static CallSite)
{
  let mut current = CALL_SITE_LL_HEAD.load(Ordering::Acquire);
  while !current.is_null()
  {
    let node = unsafe { &*current };
    f(node);
    current = node.next.load(Ordering::Acquire);
  }
}

/// Report of the profiler state at a given point in time
pub struct Report
{
  call_sites: Vec<CallSiteReport>,
}

impl Report
{
  pub(crate) const fn new(call_sites: Vec<CallSiteReport>) -> Self
  {
    Self { call_sites }
  }

  /// Returns the call site reports in the report.
  pub fn call_sites(&self) -> &[CallSiteReport]
  {
    &self.call_sites
  }
}

pub struct CallSiteReport
{
  name: &'static str,
  file: &'static str,
  line: u32,
  thread_reports: Vec<ThreadReport>,
}

impl CallSiteReport
{
  pub(crate) const fn new(name: &'static str, file: &'static str, line: u32, thread_reports: Vec<ThreadReport>) -> Self
  {
    Self { name, file, line, thread_reports }
  }

  /// Returns the name of the call site.
  pub const fn name(&self) -> &'static str
  {
    self.name
  }

  /// Returns the file name of the call site.
  pub const fn file(&self) -> &'static str
  {
    self.file
  }

  /// Returns the line number of the call site.
  pub const fn line(&self) -> u32
  {
    self.line
  }

  /// Returns the thread reports for this call site.
  pub fn thread_reports(&self) -> &[ThreadReport]
  {
    &self.thread_reports
  }
}

pub struct ThreadReport
{
  thread_name: &'static str,
  count: u64,
  total_time: u64,
  max_time: u64,
  bins: Box<[u64]>
}

impl ThreadReport
{
  pub(crate) fn new(thread_name: &'static str, count: u64, total_time: u64, max_time: u64, bins: Box<[u64]>) -> Self
  {
    Self { thread_name, count, total_time, max_time, bins }
  }

  /// Returns the name of the thread.
  pub const fn thread_name(&self) -> &'static str
  {
    self.thread_name
  }

  /// Returns the number of times the call site was hit in this thread.
  pub const fn count(&self) -> u64
  {
    self.count
  }

  /// Returns the total time spent in the call site in this thread.
  pub const fn total_time(&self) -> u64
  {
    self.total_time
  }

  /// Returns the maximum time spent in the call site in this thread.
  pub const fn max_time(&self) -> u64
  {
    self.max_time
  }

  /// Returns the raw histogram of time spent in the call site in this thread, with each bin representing a range of time.
  pub const fn bins(&self) -> &[u64]
  {
    &self.bins
  }
}

/// Builds a report of the current profiler state, including all call sites and their associated thread reports.
pub fn build_report() -> Report
{
  let mut call_site_reports = Vec::new();
  for_each_call_site(|call_site| 
  {
    let mut thread_reports = Vec::new();
    for_each_thread_aggregate(|thread_aggregate| 
    {
      let snapshot = thread_aggregate.snapshot(call_site.id());
      if snapshot.count > 0
      {
        thread_reports.push(ThreadReport::new(
          &thread_aggregate.thread_name,
          snapshot.count,
          snapshot.total_time,
          snapshot.max_time,
          snapshot.bins
        ));
      }
    });
    if !thread_reports.is_empty()
    {
      call_site_reports.push(CallSiteReport::new(
        call_site.name(),
        call_site.file(),
        call_site.line(),
        thread_reports,
      ));
    }
  });
  Report::new(call_site_reports)
}

struct ThreadHandle(&'static ThreadAggregate);

impl Drop for ThreadHandle
{
  fn drop(&mut self)
  {
    self.0.mark_free();
  }
}

thread_local!
{
  static THREAD_AGGREGATE: ThreadHandle = ThreadHandle(register_thread_aggregate());
}

static THREAD_AGGREGATE_LL_HEAD: AtomicPtr<ThreadAggregate> = AtomicPtr::new(std::ptr::null_mut());

pub(crate) fn register_thread_aggregate() -> &'static ThreadAggregate
{
  clock::init();

  let current_thread = std::thread::current();
  let thread_name = current_thread.name().unwrap_or("unnamed");

  let mut current = THREAD_AGGREGATE_LL_HEAD.load(Ordering::Acquire);
  while !current.is_null()
  {
    let node = unsafe { &*current };
    if node.thread_name == thread_name && node.cas_free()
    {
      return node;
    }
    current = node.next.load(Ordering::Acquire);
  }

  let config = *CONFIG.get_or_init(ProfilerConfiguration::default);
  let new_node = Box::into_raw(Box::new(ThreadAggregate::new_pre_occupied(config, thread_name)));
  let mut head = THREAD_AGGREGATE_LL_HEAD.load(Ordering::Relaxed);
  loop
  {
    unsafe { (*new_node).next.store(head, Ordering::Relaxed) };
    match THREAD_AGGREGATE_LL_HEAD.compare_exchange_weak(head, new_node, Ordering::Release, Ordering::Relaxed)
    {
      Ok(_) => return unsafe { &*new_node },
      Err(actual) => head = actual,
    }
  }
}

static CALL_SITE_ID_COUNTER: AtomicU32 = AtomicU32::new(0);
static CALL_SITE_LL_HEAD: AtomicPtr<CallSite> = AtomicPtr::new(std::ptr::null_mut());

pub struct CallSite
{
  id: AtomicU32,
  name: &'static str,
  file: &'static str,
  line: u32,
  next: AtomicPtr<CallSite>
}

impl CallSite
{
  pub const fn new(name: &'static str, file: &'static str, line: u32) -> Self
  {
    Self 
    {
      id: AtomicU32::new(0),
      name,
      file,
      line,
      next: AtomicPtr::new(std::ptr::null_mut())
    }
  }

  /// Returns the unique ID of the call site, initializing it if necessary.
  #[inline(always)]
  pub fn id(&'static self) -> u32
  {
    let loaded_id = self.id.load(Ordering::Relaxed);
    if loaded_id == 0
    {
      return self.init()
    }
    else {
      loaded_id
    }
  }

  #[cold]
  fn init(&'static self) -> u32
  {
    loop 
    {
      let loaded_id = self.id.load(Ordering::Relaxed);
      if loaded_id != 0
      {
        return loaded_id
      }
      let new_id = CALL_SITE_ID_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
      match self.id.compare_exchange(0, new_id, Ordering::Release, Ordering::Relaxed)
      {
        Ok(_) => {
          let mut head = CALL_SITE_LL_HEAD.load(Ordering::Relaxed);
          loop
          {
            self.next.store(head, Ordering::Relaxed);
            // no UB from the const -> mut cast, here, its always dereffed as a &'static CallSite 
            match CALL_SITE_LL_HEAD.compare_exchange_weak(head, self as *const _ as *mut _, Ordering::Release, Ordering::Relaxed)
            {
              Ok(_) => break,
              Err(actual) => head = actual,
            }
          }
          return new_id
        },
        Err(_) => continue,
      }
    }
  }

  /// Returns the name of the call site.
  #[inline(always)]
  pub const fn name(&self) -> &'static str
  {
    self.name
  }

  /// Returns the file name of the call site.
  #[inline(always)]
  pub const fn file(&self) -> &'static str
  {
    self.file
  }

  /// Returns the line number of the call site.
  #[inline(always)]
  pub const fn line(&self) -> u32
  {
    self.line
  }
}

pub struct Guard<'a>
{
  site: &'static CallSite,
  start: clock::Stamp,
  _phantom: std::marker::PhantomData<&'a ()>
}

impl<'a> Guard<'a>
{
  fn new(site: &'static CallSite) -> Self
  {
    Self 
    {
      site,
      start: clock::tick(),
      _phantom: std::marker::PhantomData,
    }
  }
}

impl<'a> Drop for Guard<'a>
{
  #[inline(always)]
  fn drop(&mut self)
  {
    let ticks = clock::ticks_since(self.start);
    let id = self.site.id();
    let _ = THREAD_AGGREGATE.try_with(|handle| handle.0.record(id, ticks));
  }
}