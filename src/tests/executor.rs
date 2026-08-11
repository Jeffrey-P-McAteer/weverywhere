use crate::executor::MessageStore;

#[test]
fn message_store_assigns_monotonic_seqs_and_filters_by_after() {
  let mut store = MessageStore::new(16);
  let s1 = store.push("alice".into(), vec![1, 2, 3], b"id1", b"hello".to_vec(), 100);
  let s2 = store.push("bob".into(), vec![4, 5, 6], b"id2", b"hi".to_vec(), 101);
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
fn message_store_dedups_repeat_deliveries_by_pubkey_and_id() {
  let mut store = MessageStore::new(16);
  let alice = vec![0xAAu8; 32];
  let bob = vec![0xBBu8; 32];
  // The same (pubkey, id) arriving several times (multicast x interfaces + unicast) is kept once.
  assert_eq!(store.push("alice".into(), alice.clone(), b"n1", b"hi".to_vec(), 1), 1);
  assert_eq!(store.push("alice".into(), alice.clone(), b"n1", b"hi".to_vec(), 1), 0, "duplicate dropped");
  assert_eq!(store.push("alice".into(), alice.clone(), b"n1", b"hi".to_vec(), 1), 0, "still dropped");
  // A different sender reusing the same id is NOT a duplicate (dedup is per pubkey).
  assert_eq!(store.push("bob".into(), bob.clone(), b"n1", b"hi".to_vec(), 1), 2);
  // Alice's genuine second message (fresh nonce) is kept even with identical text.
  assert_eq!(store.push("alice".into(), alice.clone(), b"n2", b"hi".to_vec(), 1), 3);
  // Empty id disables dedup entirely (non-chat callers).
  assert_eq!(store.push("sys".into(), vec![], b"", b"x".to_vec(), 1), 4);
  assert_eq!(store.push("sys".into(), vec![], b"", b"x".to_vec(), 1), 5);
  assert_eq!(store.read_after(0).len(), 5);
}

#[test]
fn message_store_evicts_oldest_past_capacity() {
  let mut store = MessageStore::new(3);
  for i in 0..5u64 {
    store.push("x".into(), vec![], b"", vec![i as u8], i);
  }
  // Only the last 3 remain, but sequence numbers keep climbing (readers still see monotonic seqs).
  let all = store.read_after(0);
  assert_eq!(all.len(), 3);
  assert_eq!(all.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
  assert_eq!(all[0].text, vec![2u8]); // the 3rd message pushed (0-indexed 2)
}
