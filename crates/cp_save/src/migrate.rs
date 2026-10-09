//! Frozen per-version save bodies and the `migrate_vN_to_vN1` chain.
//!
//! A `SaveDataV<N>` type is a snapshot of `SaveData` as it was at version `N` and must
//! never change (its postcard encoding is what old files contain; `SimCommand` and the
//! domain enums are append-only for the same reason).

use cp_building::BuildingGrid;
use cp_citizen::{CitizenGrid, TripBook};
use cp_core::{CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_net::NetGraph;
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;
use cp_traffic::SavedPaths;
use cp_utility::UtilityGraph;
use cp_zone::ZoneGrid;
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
pub fn migrate_v2_to_v3(v2: SaveDataV2) -> SaveDataV3 {
    SaveDataV3 {
        tick: v2.tick,
        rng: v2.rng,
        speed: v2.speed,
        queue: v2.queue,
        log: v2.log,
        tiles: v2.tiles,
        net: NetGraph::default(),
    }
}

/// Version 3: the body of `SaveData` as written by format 3 (body-only CRC, no map hash).
/// Version 4 changed only the header, so the bodies are identical.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV3 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
}

/// Version 4: the body as written by format 4 (before zone state).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV4 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
}

/// Version 5: the body as written by format 5 (zone state, before buildings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV5 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: ZoneGrid,
}

/// Version 6: the body as written by format 6 (buildings, before citizens).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV6 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: ZoneGrid,
    pub buildings: BuildingGrid,
}

/// Version 7: the body as written by format 7 (citizens, before pending path requests).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV7 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: ZoneGrid,
    pub buildings: BuildingGrid,
    pub citizens: CitizenGrid,
}

/// Version 8: the body as written by format 8 (pending path requests, before utilities).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV8 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: ZoneGrid,
    pub buildings: BuildingGrid,
    pub citizens: CitizenGrid,
    pub paths: SavedPaths,
}

/// Version 9: the body as written by format 9 (utility networks, before trips).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveDataV9 {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
    pub net: NetGraph,
    pub zones: ZoneGrid,
    pub buildings: BuildingGrid,
    pub citizens: CitizenGrid,
    pub paths: SavedPaths,
    pub utilities: UtilityGraph,
}

/// v9 → v10: trips were added; an old save has none in flight (citizens simply decide
/// again at their next visit).
pub fn migrate_v9_to_v10(v9: SaveDataV9) -> SaveData {
    SaveData {
        tick: v9.tick,
        rng: v9.rng,
        speed: v9.speed,
        queue: v9.queue,
        log: v9.log,
        tiles: v9.tiles,
        net: v9.net,
        zones: v9.zones,
        buildings: v9.buildings,
        citizens: v9.citizens,
        paths: v9.paths,
        utilities: v9.utilities,
        trips: TripBook::default(),
    }
}

/// v8 → v9: the utility networks (pipes, power lines) were added; an old save has none.
pub fn migrate_v8_to_v9(v8: SaveDataV8) -> SaveDataV9 {
    SaveDataV9 {
        tick: v8.tick,
        rng: v8.rng,
        speed: v8.speed,
        queue: v8.queue,
        log: v8.log,
        tiles: v8.tiles,
        net: v8.net,
        zones: v8.zones,
        buildings: v8.buildings,
        citizens: v8.citizens,
        paths: v8.paths,
        utilities: UtilityGraph::default(),
    }
}

/// v7 → v8: pending path requests were added; an old save has none in flight and the next
/// request id restarts at 0.
pub fn migrate_v7_to_v8(v7: SaveDataV7) -> SaveDataV8 {
    SaveDataV8 {
        tick: v7.tick,
        rng: v7.rng,
        speed: v7.speed,
        queue: v7.queue,
        log: v7.log,
        tiles: v7.tiles,
        net: v7.net,
        zones: v7.zones,
        buildings: v7.buildings,
        citizens: v7.citizens,
        paths: SavedPaths::default(),
    }
}

/// v6 → v7: citizens and households were added; an old save has none (its homes are
/// empty and move-ins start again).
pub fn migrate_v6_to_v7(v6: SaveDataV6) -> SaveDataV7 {
    SaveDataV7 {
        tick: v6.tick,
        rng: v6.rng,
        speed: v6.speed,
        queue: v6.queue,
        log: v6.log,
        tiles: v6.tiles,
        net: v6.net,
        zones: v6.zones,
        buildings: v6.buildings,
        citizens: CitizenGrid::default(),
    }
}

/// v5 → v6: buildings were added; an old save has none (its zones keep their painted
/// types and every valid cell is free).
pub fn migrate_v5_to_v6(v5: SaveDataV5) -> SaveDataV6 {
    SaveDataV6 {
        tick: v5.tick,
        rng: v5.rng,
        speed: v5.speed,
        queue: v5.queue,
        log: v5.log,
        tiles: v5.tiles,
        net: v5.net,
        zones: v5.zones,
        buildings: BuildingGrid::default(),
    }
}

/// v4 → v5: zone state was added; an old save starts with an empty [`ZoneGrid`] and the
/// `Network` phase builds the blocks from the loaded road network on the first tick.
pub fn migrate_v4_to_v5(v4: SaveDataV4) -> SaveDataV5 {
    SaveDataV5 {
        tick: v4.tick,
        rng: v4.rng,
        speed: v4.speed,
        queue: v4.queue,
        log: v4.log,
        tiles: v4.tiles,
        net: v4.net,
        zones: ZoneGrid::default(),
    }
}

/// v3 → v4: header-only change (CRC over the whole file, map hash); the body is carried
/// over unchanged.
pub fn migrate_v3_to_v4(v3: SaveDataV3) -> SaveDataV4 {
    SaveDataV4 {
        tick: v3.tick,
        rng: v3.rng,
        speed: v3.speed,
        queue: v3.queue,
        log: v3.log,
        tiles: v3.tiles,
        net: v3.net,
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
    // One linear chain: every arm decodes its version's body and then applies each
    // remaining `migrate_vN_to_vN1` step in order.
    match version {
        0 => {
            let v = migrate_v0_to_v1(dec(body)?);
            let v = migrate_v2_to_v3(v);
            let v = migrate_v3_to_v4(v);
            let v = migrate_v4_to_v5(v);
            let v = migrate_v5_to_v6(v);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(migrate_v9_to_v10(migrate_v8_to_v9(v)))
        }
        // v1 and v2 share the same body; v2 only added the header CRC.
        1 | 2 => {
            let v = migrate_v2_to_v3(dec(body)?);
            let v = migrate_v3_to_v4(v);
            let v = migrate_v4_to_v5(v);
            let v = migrate_v5_to_v6(v);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(migrate_v9_to_v10(migrate_v8_to_v9(v)))
        }
        3 => {
            let v = migrate_v3_to_v4(dec(body)?);
            let v = migrate_v4_to_v5(v);
            let v = migrate_v5_to_v6(v);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(migrate_v9_to_v10(migrate_v8_to_v9(v)))
        }
        4 => {
            let v = migrate_v4_to_v5(dec(body)?);
            let v = migrate_v5_to_v6(v);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(migrate_v9_to_v10(migrate_v8_to_v9(v)))
        }
        5 => {
            let v = migrate_v5_to_v6(dec(body)?);
            let v = migrate_v6_to_v7(v);
            let v = migrate_v7_to_v8(v);
            Ok(migrate_v9_to_v10(migrate_v8_to_v9(v)))
        }
        6 => {
            let v = migrate_v6_to_v7(dec(body)?);
            let v = migrate_v7_to_v8(v);
            Ok(migrate_v9_to_v10(migrate_v8_to_v9(v)))
        }
        7 => Ok(migrate_v9_to_v10(migrate_v8_to_v9(migrate_v7_to_v8(dec(
            body,
        )?)))),
        8 => Ok(migrate_v9_to_v10(migrate_v8_to_v9(dec(body)?))),
        9 => Ok(migrate_v9_to_v10(dec(body)?)),
        10 => dec(body),
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
            map_hash: None,
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
            map_hash: None,
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
            map_hash: None,
            body_crc: Some(crate::crc32(&frame)),
        },
    )?;
    out.extend_from_slice(&frame);
    Ok(out)
}

/// Writes a **version 3** file (body-only CRC, no map hash). Legacy writer kept only to
/// (re)generate and test the v3 fixture; real saves always use [`crate::save`].
pub fn encode_v3(v3: &SaveDataV3, content_hash: u64, map_id: &str) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v3).expect("serialization failed");
    let frame = crate::compress(&body);
    let mut out = Vec::new();
    crate::write_header(
        &mut out,
        &crate::Header {
            format_version: 3,
            content_hash,
            tick: v3.tick.0,
            map_id: map_id.to_owned(),
            map_hash: None,
            body_crc: Some(crate::crc32(&frame)),
        },
    )?;
    out.extend_from_slice(&frame);
    Ok(out)
}

/// Writes a **version 4** file (whole-file CRC and map hash, no zone state). Legacy writer
/// kept only to regenerate and test the v4 fixture; real saves always use [`crate::save`].
pub fn encode_v4(
    v4: &SaveDataV4,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v4).expect("serialization failed");
    crate::assemble(4, v4.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 5** file (zone state, no buildings). Legacy writer kept only to
/// regenerate and test the v5 fixture; real saves always use [`crate::save`].
pub fn encode_v5(
    v5: &SaveDataV5,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v5).expect("serialization failed");
    crate::assemble(5, v5.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 6** file (buildings, no citizens). Legacy writer kept only to
/// regenerate and test the v6 fixture; real saves always use [`crate::save`].
pub fn encode_v6(
    v6: &SaveDataV6,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v6).expect("serialization failed");
    crate::assemble(6, v6.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 7** file (citizens, no pending path requests). Legacy writer kept only
/// to regenerate and test the v7 fixture; real saves always use [`crate::save`].
pub fn encode_v7(
    v7: &SaveDataV7,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v7).expect("serialization failed");
    crate::assemble(7, v7.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 8** file (pending path requests, no utilities). Legacy writer kept
/// only to (re)generate and test the v8 fixture; real saves always use [`crate::save`].
pub fn encode_v8(
    v8: &SaveDataV8,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v8).expect("serialization failed");
    crate::assemble(8, v8.tick.0, &body, content_hash, map_id, map_hash)
}

/// Writes a **version 9** file (utility networks, no trips). Legacy writer kept only to
/// regenerate and test the v9 fixture; real saves always use [`crate::save`].
pub fn encode_v9(
    v9: &SaveDataV9,
    content_hash: u64,
    map_id: &str,
    map_hash: Option<u64>,
) -> Result<Vec<u8>, SaveError> {
    let body = postcard::to_stdvec(v9).expect("serialization failed");
    crate::assemble(9, v9.tick.0, &body, content_hash, map_id, map_hash)
}
