use super::*;

type Container = AggregateStatBucketContainer<BUCKETS>;

fn aggregate() -> ThreadAggregate
{
  ThreadAggregate::new_pre_occupied(ProfilerConfiguration::default(), "test")
}

#[test]
fn slots_map_to_consecutive_bucket_positions()
{
  let (mut bucket, mut offset) = (0, 0);
  for slot in 0..1_000_000
  {
    let actual_bucket = Container::bucket_index(slot);
    let actual_offset = Container::slot_in_bucket(slot, Container::bucket_size(actual_bucket));
    assert_eq!((actual_bucket, actual_offset), (bucket, offset), "slot {slot}");
    offset += 1;
    if offset == Container::bucket_size(bucket)
    {
      bucket += 1;
      offset = 0;
    }
  }
}

#[test]
fn samples_land_in_the_expected_bin()
{
  let cases: [(u64, usize); 12] =
  [
    (0, 0), (1, 0), (511, 0), (512, 1), (1023, 1), (1024, 2),
    ((1 << 22) - 1, 13), (1 << 22, 14), ((1 << 23) - 1, 14), (1 << 23, 15), (1 << 40, 15), (u64::MAX, 15),
  ];
  for (time, expected) in cases
  {
    let aggregate = aggregate();
    aggregate.record(1, time);
    let snapshot = aggregate.snapshot(1);
    let mut bins = [0u64; 16];
    bins[expected] = 1;
    assert_eq!(*snapshot.bins, bins, "time {time}");
    assert_eq!(snapshot.total_time, time);
  }
}

#[test]
fn max_keeps_the_largest_sample()
{
  let aggregate = aggregate();
  for time in [500, 9000, 300]
  {
    aggregate.record(1, time);
  }
  assert_eq!(aggregate.snapshot(1).max_time, 9000);
}

#[test]
fn aggregate_cas_free()
{
  let aggregate = aggregate();
  assert!(!aggregate.cas_free());
  aggregate.mark_free();
  assert!(aggregate.cas_free());
  assert!(!aggregate.cas_free());
}

#[test]
#[should_panic]
fn zero_bins_rejected()
{
  ThreadAggregateConfig::new(0, 8);
}

#[test]
#[should_panic]
fn invalid_bins_rejected()
{
  ThreadAggregateConfig::new(65, 8);
}