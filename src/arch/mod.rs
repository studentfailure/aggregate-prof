#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(not(target_arch = "x86_64"))]
pub mod generic;

pub mod clock
{
  #[cfg(target_arch = "x86_64")]
  pub use crate::arch::x86_64::clock::*;
  #[cfg(not(target_arch = "x86_64"))]
  pub use crate::arch::generic::clock::*;
}