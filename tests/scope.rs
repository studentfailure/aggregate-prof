#![cfg_attr(feature = "nightly", feature(const_type_name))]

use aggregate_prof::{build_report, scope};
use std::thread::{self, Builder};

fn thread_counts(site_name: &str) -> Vec<(&'static str, u64)>
{
  let report = build_report();
  let mut counts: Vec<_> = report.call_sites().iter()
    .filter(|site| site.name() == site_name)
    .flat_map(|site| site.thread_reports().iter().map(|thread| (thread.thread_name(), thread.count())))
    .collect();
  counts.sort();
  counts
}

fn run_named(name: &str, f: fn())
{
  Builder::new().name(name.to_string()).spawn(f).unwrap().join().unwrap();
}

#[cfg(feature = "nightly")]
#[test]
fn derived_scope_name_from_fn()
{
  let fn_name = aggregate_prof::_psuedo_fn_name!();
  assert!(fn_name.ends_with("derived_scope_name_from_fn"));
}

#[test]
fn several_scopes_can_share_a_block()
{
  fn work()
  {
    scope!("first_in_block");
    scope!("second_in_block");
  }
  run_named("block", work);
  assert_eq!(thread_counts("first_in_block"), [("block", 1)]);
  assert_eq!(thread_counts("second_in_block"), [("block", 1)]);
}

#[test]
fn named_thread_variance()
{
  fn work()
  {
    scope!("apart");
  }
  run_named("apart-a", work);
  run_named("apart-b", work);
  assert_eq!(thread_counts("apart"), [("apart-a", 1), ("apart-b", 1)]);
}

#[test]
fn same_named_threads_share_aggregate_once_freed()
{
  fn work()
  {
    scope!("shared");
  }
  run_named("shared-worker", work);
  run_named("shared-worker", work);
  assert_eq!(thread_counts("shared"), [("shared-worker", 2)]);
}

#[test]
fn scope_in_a_tls_destructor_does_not_panic()
{
  struct ScopeOnDrop;

  impl Drop for ScopeOnDrop
  {
    fn drop(&mut self)
    {
      scope!("scope_on_drop");
    }
  }

  thread_local!
  {
    static ON_EXIT: ScopeOnDrop = const { ScopeOnDrop };
  }

  thread::spawn(|| ON_EXIT.with(|_| ())).join().unwrap();
}
