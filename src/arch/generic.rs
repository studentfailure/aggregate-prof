pub mod clock
{
  use std::time::Instant;

  pub type Stamp = Instant;

  #[inline(always)]
  pub fn tick() -> Stamp
  {
    Instant::now() 
  }

  #[inline(always)]
  pub fn ticks_since(start: Stamp) -> u64
  {
    start.elapsed().as_nanos() as u64
  }

  pub const fn init()
  {
  }

  pub const fn ticks_per_ns() -> f64
  {
    1.0
  }

  pub const fn to_nanos(ticks: u64) -> u64
  {
    ticks as f64
  }
}