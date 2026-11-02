pub mod clock
{
  use std::{sync::OnceLock, time::Instant};

  pub type Stamp = u64;
  
  static ANCHOR: OnceLock<(Instant, u64)> = OnceLock::new();

  #[inline(always)]
  pub fn tick() -> Stamp
  {
    unsafe { std::arch::x86_64::_rdtsc() }  
  }

  #[inline(always)]
  pub fn ticks_since(start: Stamp) -> u64
  {
    tick().saturating_sub(start)
  }

  pub fn init()
  {
    ANCHOR.get_or_init(|| (Instant::now(), tick()));
  }

  pub fn ticks_per_ns() -> f64
  {
    let (start_time, start_ticks) = *ANCHOR.get_or_init(|| (Instant::now(), tick()));
    let ticks = tick().saturating_sub(start_ticks);
    ticks as f64 / start_time.elapsed().as_nanos() as f64
  }

  pub fn ticks_to_nanos(ticks: u64) -> u64
  {
    (ticks as f64 / ticks_per_ns()) as u64
  }
}