use super::*;

#[test]
fn request_layout_matches_the_go_reader() {
    // dispatch.go reads: reqId u32, methodLen u16, method, payloadLen u32, payload.
    let frame = encode_request(0x0A0B0C0D, "Stop", &[0xAA, 0xBB]);
    assert_eq!(&frame[0..4], &[0x0D, 0x0C, 0x0B, 0x0A]); // little-endian id
    assert_eq!(&frame[4..6], &[4, 0]); // method length
    assert_eq!(&frame[6..10], b"Stop");
    assert_eq!(&frame[10..14], &[2, 0, 0, 0]); // payload length
    assert_eq!(&frame[14..], &[0xAA, 0xBB]);
}

#[test]
fn empty_payload_still_carries_a_length() {
    let frame = encode_request(1, "QueryStats", &[]);
    assert_eq!(frame.len(), 4 + 2 + 10 + 4);
    assert_eq!(&frame[16..20], &[0, 0, 0, 0]);
}

#[tokio::test]
async fn reads_a_well_formed_response() {
    let mut wire: Vec<u8> = Vec::new();
    wire.extend_from_slice(&7u32.to_le_bytes());
    wire.push(STATUS_OK);
    wire.extend_from_slice(&3u32.to_le_bytes());
    wire.extend_from_slice(&[1, 2, 3]);

    let resp = read_response(&mut wire.as_slice()).await.unwrap();
    assert_eq!(resp.id, 7);
    assert_eq!(resp.into_result().unwrap(), vec![1, 2, 3]);
}

#[tokio::test]
async fn error_status_surfaces_the_payload_as_text() {
    let msg = b"unknown method: Nope";
    let mut wire: Vec<u8> = Vec::new();
    wire.extend_from_slice(&9u32.to_le_bytes());
    wire.push(STATUS_ERR);
    wire.extend_from_slice(&(msg.len() as u32).to_le_bytes());
    wire.extend_from_slice(msg);

    let resp = read_response(&mut wire.as_slice()).await.unwrap();
    assert_eq!(resp.into_result().unwrap_err(), "unknown method: Nope");
}

#[tokio::test]
async fn oversized_frame_is_refused_before_allocation() {
    let mut wire: Vec<u8> = Vec::new();
    wire.extend_from_slice(&1u32.to_le_bytes());
    wire.push(STATUS_OK);
    wire.extend_from_slice(&u32::MAX.to_le_bytes());

    let err = read_response(&mut wire.as_slice()).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}
