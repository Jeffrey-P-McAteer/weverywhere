use crate::messages::NetworkMessage;

#[test]
fn basic_return_map_round_trips_over_bare() {
  // Confirms serde_bare handles the new variant, including the fixed [u8; 16] request UUID,
  // and that appending variants didn't disturb the existing ones.
  let uuid = [7u8; 16];
  let msg = NetworkMessage::BasicReturnMap {
    from_pid: 42,
    request_uuid: uuid,
    cbor_data: vec![0xa1, 0x01, 0x02], // a tiny CBOR map {1: 2}
  };
  let bytes = serde_bare::to_vec(&msg).expect("encode");
  match serde_bare::from_slice::<NetworkMessage>(&bytes).expect("decode") {
    NetworkMessage::BasicReturnMap { from_pid, request_uuid, cbor_data } => {
      assert_eq!(from_pid, 42);
      assert_eq!(request_uuid, uuid);
      assert_eq!(cbor_data, vec![0xa1, 0x01, 0x02]);
    }
    other => panic!("wrong variant: {other:?}"),
  }
}

#[test]
fn cbor_map_and_list_round_trip() {
  // Sanity that serde_cbor (already a dependency) encodes/decodes the map + list payloads that
  // BasicReturnMap / BasicReturnList carry.
  use serde_cbor::Value;
  let map = Value::Map(
    [(Value::Text("hostname".into()), Value::Text("node1".into()))]
      .into_iter()
      .collect(),
  );
  let enc = serde_cbor::to_vec(&map).expect("cbor map encode");
  assert_eq!(serde_cbor::from_slice::<Value>(&enc).expect("cbor map decode"), map);

  let list = Value::Array(vec![Value::Integer(1), Value::Text("a".into())]);
  let enc = serde_cbor::to_vec(&list).expect("cbor list encode");
  assert_eq!(serde_cbor::from_slice::<Value>(&enc).expect("cbor list decode"), list);
}
