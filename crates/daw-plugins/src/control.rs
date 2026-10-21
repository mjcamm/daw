//! The control channel: everything that is not audio, over the host
//! process's stdin and stdout as length-prefixed JSON. Never used on the
//! audio thread.

use crate::block::{Layout, Names};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Bump when messages change; the host refuses a different one.
pub const PROTOCOL: u32 = 3;

/// Largest message accepted, so a garbled length can't allocate gigabytes.
const MAX_MESSAGE: u32 = 64 << 20;

pub use daw_protocol::{PluginData, PluginParamInfo, PluginSpec, TestPlugin};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Starts an instance, from saved state if there is some.
    Create {
        instance: u64,
        device: PluginSpec,
        state: Option<PluginData>,
    },
    /// Opens the instance's audio channel (made by the DAW) and starts its
    /// audio thread.
    Activate {
        instance: u64,
        names: Names,
        layout: Layout,
        sample_rate: f64,
    },
    Deactivate {
        instance: u64,
    },
    Destroy {
        instance: u64,
    },
    /// The instance's state, to save with the song ([`Reply::State`]).
    GetState {
        instance: u64,
    },
    /// The instance's automatable parameters ([`Reply::Params`]).
    GetParams {
        instance: u64,
    },
}

/// What a call answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Done,
    /// `None` for a device without state.
    State(Option<PluginData>),
    Params(Vec<PluginParamInfo>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToHost {
    Hello {
        protocol: u32,
    },
    Call {
        id: u64,
        request: Request,
    },
    /// Stop everything and exit.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FromHost {
    Ready {
        protocol: u32,
        pid: u32,
    },
    Reply {
        id: u64,
        result: Result<Reply, String>,
    },
}

pub fn write_message(to: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    let len = u32::try_from(bytes.len())
        .ok()
        .filter(|&n| n <= MAX_MESSAGE)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "message too large"))?;
    to.write_all(&len.to_le_bytes())?;
    to.write_all(&bytes)?;
    to.flush()
}

/// The next message, or `None` when the other side closed the channel.
pub fn read_message<T: DeserializeOwned>(from: &mut impl Read) -> io::Result<Option<T>> {
    let mut len = [0; 4];
    match from.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut bytes = vec![0; len as usize];
    from.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_and_end_cleanly() {
        let mut wire = Vec::new();
        let sent = ToHost::Call {
            id: 3,
            request: Request::Create {
                instance: 9,
                device: PluginSpec::Test(TestPlugin::Slow { millis: 2 }),
                state: Some(PluginData {
                    component: vec![1, 2, 3],
                    controller: Vec::new(),
                }),
            },
        };
        write_message(&mut wire, &sent).unwrap();
        write_message(&mut wire, &ToHost::Shutdown).unwrap();
        let mut from = wire.as_slice();
        assert_eq!(read_message::<ToHost>(&mut from).unwrap(), Some(sent));
        assert_eq!(
            read_message::<ToHost>(&mut from).unwrap(),
            Some(ToHost::Shutdown)
        );
        assert_eq!(read_message::<ToHost>(&mut from).unwrap(), None);
    }

    #[test]
    fn a_garbled_length_is_refused_without_allocating_it() {
        let wire = u32::MAX.to_le_bytes();
        assert!(read_message::<ToHost>(&mut wire.as_slice()).is_err());
    }
}
