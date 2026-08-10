use crate::executor::MessageStore;

#[test]
fn message_store_assigns_monotonic_seqs_and_filters_by_after() {
  let mut store = MessageStore::new(16);
  let s1 = store.push("alice".into(), vec![1, 2, 3], b"hello".to_vec(), 100);
  let s2 = store.push("bob".into(), vec![4, 5, 6], b"hi".to_vec(), 101);
  assert_eq!((s1, s2), (1, 2), "seqs start at 1 and increase");

  // read_after(0) returns everything; read_after(1) skips the first; read_after(2) is empty.
  assert_eq!(store.read_after(0).len(), 2);
  let after1 = store.read_after(1);
  assert_eq!(after1.len(), 1);
  assert_eq!(after1[0].from_name, "bob");
  assert_eq!(after1[0].text, b"hi");
  assert!(store.read_after(2).is_empty());
}

#[test]
fn message_store_evicts_oldest_past_capacity() {
  let mut store = MessageStore::new(3);
  for i in 0..5u64 {
    store.push("x".into(), vec![], vec![i as u8], i);
  }
  // Only the last 3 remain, but sequence numbers keep climbing (readers still see monotonic seqs).
  let all = store.read_after(0);
  assert_eq!(all.len(), 3);
  assert_eq!(all.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
  assert_eq!(all[0].text, vec![2u8]); // the 3rd message pushed (0-indexed 2)
}
