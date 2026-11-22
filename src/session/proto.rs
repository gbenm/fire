//! Framing for the control socket (`ctl.sock`) between session clients and the supervisor.
//!
//! Each frame is `[kind: u8][len: u32 big-endian][payload]`.

use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
};

/// Client -> supervisor: raw bytes written to the pty.
pub(crate) const INPUT: u8 = 1;
/// Client -> supervisor: `rows: u16, cols: u16`.
pub(crate) const RESIZE: u8 = 2;
/// Client -> supervisor: `signal: i32`.
pub(crate) const SIGNAL: u8 = 3;
/// Client -> supervisor: `rows: u16, cols: u16, since: u64`. Starts streaming output to this
/// client, replaying the log from `since` (or a short tail when `since` is [`TAIL`]).
pub(crate) const ATTACH: u8 = 4;

/// Supervisor -> client: `offset: u64`, the log offset of the first byte streamed next.
pub(crate) const HELLO: u8 = 0x81;
/// Supervisor -> client: raw pty output.
pub(crate) const OUTPUT: u8 = 0x82;
/// Supervisor -> client: `code: i32`. The program exited.
pub(crate) const EXIT: u8 = 0x83;

pub(crate) const TAIL: u64 = u64::MAX;

const HEADER: usize = 5;
const MAX_PAYLOAD: usize = 4 * 1024 * 1024;

pub(crate) fn encode(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Takes the first complete frame out of `buffer`. `Err` means the peer sent garbage.
pub(crate) fn decode(buffer: &mut Vec<u8>) -> Result<Option<(u8, Vec<u8>)>, ()> {
    if buffer.len() < HEADER {
        return Ok(None);
    }
    let len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(());
    }
    if buffer.len() < HEADER + len {
        return Ok(None);
    }
    let kind = buffer[0];
    let payload = buffer[HEADER..HEADER + len].to_vec();
    buffer.drain(..HEADER + len);
    Ok(Some((kind, payload)))
}

pub(crate) fn size_payload(rows: u16, cols: u16) -> Vec<u8> {
    let mut payload = rows.to_be_bytes().to_vec();
    payload.extend_from_slice(&cols.to_be_bytes());
    payload
}

pub(crate) fn attach_payload(rows: u16, cols: u16, since: u64) -> Vec<u8> {
    let mut payload = size_payload(rows, cols);
    payload.extend_from_slice(&since.to_be_bytes());
    payload
}

pub(crate) fn read_u16(payload: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        payload.get(at..at + 2)?.try_into().ok()?,
    ))
}

pub(crate) fn read_i32(payload: &[u8]) -> Option<i32> {
    Some(i32::from_be_bytes(payload.get(..4)?.try_into().ok()?))
}

pub(crate) fn read_u64(payload: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        payload.get(at..at + 8)?.try_into().ok()?,
    ))
}

pub(crate) fn send(stream: &mut UnixStream, kind: u8, payload: &[u8]) -> io::Result<()> {
    stream.write_all(&encode(kind, payload))
}

/// Reads whatever is available into `buffer`. `Ok(false)` means the peer closed the socket.
pub(crate) fn fill(stream: &mut UnixStream, buffer: &mut Vec<u8>) -> io::Result<bool> {
    let mut chunk = [0u8; 64 * 1024];
    match stream.read(&mut chunk) {
        Ok(0) => Ok(false),
        Ok(read) => {
            buffer.extend_from_slice(&chunk[..read]);
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::Interrupted => Ok(true),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_wait_for_complete_payloads() {
        let mut buffer = encode(INPUT, b"hello");
        buffer.extend(encode(RESIZE, &size_payload(40, 120)));
        let partial = buffer.split_off(buffer.len() - 2);

        assert_eq!(decode(&mut buffer), Ok(Some((INPUT, b"hello".to_vec()))));
        assert_eq!(decode(&mut buffer), Ok(None));
        buffer.extend(partial);
        let (kind, payload) = decode(&mut buffer).unwrap().unwrap();
        assert_eq!(kind, RESIZE);
        assert_eq!(read_u16(&payload, 0), Some(40));
        assert_eq!(read_u16(&payload, 2), Some(120));
        assert!(buffer.is_empty());
    }

    #[test]
    fn rejects_oversized_frames() {
        let mut buffer = vec![INPUT, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(decode(&mut buffer), Err(()));
    }

    #[test]
    fn attach_payload_carries_offset() {
        let payload = attach_payload(24, 80, TAIL);
        assert_eq!(read_u64(&payload, 4), Some(TAIL));
    }
}
