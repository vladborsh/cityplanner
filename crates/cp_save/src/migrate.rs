//! Frozen per-version save bodies and the `migrate_vN_to_vN1` chain.
//!
//! A `SaveDataV<N>` type is a snapshot of `SaveData` as it was at version `N` and must
//! never change (its postcard encoding is what old files contain; `SimCommand` and the
//! domain enums are append-only for the same reason).

use cp_core::{CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_net::NetGraph;
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;
use serde::{Deserialize, Serialize};

use crate::{SaveData, SaveError};

/// Version 0: before `SimSpeed` was saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV0 {
    pub tick: Tick,
    pub rng: SimRng,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
}

/// Versions 1 and 2 (they share one body; v2 only added the header CRC): before the road
/// network (`NetGraph`) was saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV2 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
}

/// v0 → v1: speed was not saved; resume at ×1, the game default.
pub fn migrate_v0_to_v1(v0: SaveDataV0) -> SaveDataV2 {
    SaveDataV2 {
        tick: v0.tick,
        rng: v0.rng,
        speed: SimSpeed::X1,
        queue: v0.queue,
        log: v0.log,
        tiles: v0.tiles,
    }
}

/// v2 → v3: no network was saved; start with an empty one.
pub fn migrate_v2_to_v3(v2: SaveDataV2) -> SaveData {
    SaveData {
        tick: v2.tick,
        rng: v2.rng,
        speed: v2.speed,
        queue: v2.queue,
        log: v2.log,
        tiles: v2.tiles,
        net: NetGraph::default(),
    }
}

/// Decodes a body written at `version` and runs the migration chain up to
/// [`crate::FORMAT_VERSION`]. Add one `match` arm per version and one chain step per bump.
pub fn decode_and_migrate(version: u32, body: &[u8]) -> Result<SaveData, SaveError> {
    fn dec<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, SaveError> {
        let (value, rest) =
            postcard::take_from_bytes(body).map_err(|e| SaveError::Decode(e.to_string()))?;
        if rest.is_empty() {
            Ok(value)
        } else {
            Err(SaveError::Decode("trailing bytes after save body".into()))
        }
    }
    match version {
        0 => Ok(migrate_v2_to_v3(migrate_v0_to_v1(dec(body)?))),
        // v1 and v2 share the same body; v2 only added the header CRC.
        1 | 2 => Ok(migrate_v2_to_v3(dec(body)?)),
        3 => dec(body),
        v => Err(SaveError::UnsupportedVersion(v)),
    }
}

/// Writes a **version 0** file. Legacy writer kept only to (re)generate and test the v0
/// fixture; real saves always use [`crate::save`].
pub fn encode_v0(v0: &SaveDataV0, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    let mut out = Vec::new();
    crate::write_header(
        &mut out,
        &crate::Header {
            format_version: 0,
            content_hash,
            tick: v0.tick.0,
            map_id: map_id.to_owned(),
            body_crc: None,
        },
    )?;
    let body = postcard::to_stdvec(v0).expect("serialization failed");
    out.extend_from_slice(&crate::compress(&body));
    Ok(out)
}

/// Writes a **version 1** file (no body CRC). Legacy writer kept only to (re)generate and
/// test the v1 fixture; real saves always use [`crate::save`].
pub fn encode_v1(v1: &SaveDataV2, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    let mut out = Vec::new();
    crate::write_header(
        &mut out,
        &crate::Header {
            format_version: 1,
            content_hash,
            tick: v1.tick.0,
            map_id: map_id.to_owned(),
            body_crc: None,
        },
    )?;
    let body = postcard::to_stdvec(v1).expect("serialization failed");
    out.extend_from_slice(&crate::compress(&body));
    Ok(out)
}

/// Writes a **version 2** file (body CRC, no network). Legacy writer kept only to
/// (re)generate and test the v2 fixture; real saves always use [`crate::save`].
pub fn encode_v2(v2: &SaveDataV2, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v2).expect("serialization failed");
    let frame = crate::compress(&body);
    let mut out = Vec::new();
    crate::write_header(
        &mut out,
        &crate::Header {
            format_version: 2,
            content_hash,
            tick: v2.tick.0,
            map_id: map_id.to_owned(),
            body_crc: Some(crate::crc32(&frame)),
        },
    )?;
    out.extend_from_slice(&frame);
    Ok(out)
}
