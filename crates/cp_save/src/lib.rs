//! `cp_save` — save format, versions and migrations (`architecture/system-design.md` §12).
//!
//! **File layout** (all integers little-endian):
//!
//! | bytes | field |
//! |---|---|
//! | 4 | magic `"CPSV"` |
//! | 4 | `format_version: u32` |
//! | 8 | `content_hash: u64` (the `ContentDb` hash the save was made with) |
//! | 8 | `tick: u64` |
//! | 2 + n | `map_id`: `u16` length, UTF-8 bytes |
//! | rest | zstd frame (pure-Rust `ruzstd`) of the postcard-encoded body |
//!
//! The header is read before the body so a version is known before decoding. Output is
//! deterministic: saving the same state twice gives identical bytes.
//!
//! **Versions and migrations.** The body of version `N` is the frozen type for `N` (see
//! [`migrate`]). Loading decodes the body at the file's version and applies
//! `migrate_vN_to_vN1` steps until it reaches [`FORMAT_VERSION`]. Shipped fixtures in
//! `fixtures/saves/` must keep loading; never edit them.
//!
//! **Adding state** (a domain's arenas, etc.): add a field to [`SaveData`], bump
//! [`FORMAT_VERSION`], freeze the previous `SaveData` as `SaveDataV<old>` in
//! [`migrate`], write `migrate_v<old>_to_v<new>` giving the new field a value, extend
//! `capture`/`restore`, and add a fixture for the old version plus a load test.

pub mod migrate;

use std::io::Read;

use bevy_ecs::world::World;
use cp_core::{CommandLog, CommandQueue, SimRng, SimSpeed, Tick};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use serde::{Deserialize, Serialize};

pub const MAGIC: [u8; 4] = *b"CPSV";
/// Current save format version.
pub const FORMAT_VERSION: u32 = 1;

/// Everything the sim needs to resume, at [`FORMAT_VERSION`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveData {
    pub tick: Tick,
    pub rng: SimRng,
    pub speed: SimSpeed,
    pub queue: CommandQueue<SimCommand>,
    pub log: CommandLog<SimCommand>,
    pub tiles: TileOwnership,
}

impl SaveData {
    /// Snapshots a runner's sim state.
    pub fn capture(world: &World) -> Self {
        Self {
            tick: *world.resource::<Tick>(),
            rng: world.resource::<SimRng>().clone(),
            speed: *world.resource::<SimSpeed>(),
            queue: world.resource::<CommandQueue<SimCommand>>().clone(),
            log: world.resource::<CommandLog<SimCommand>>().clone(),
            tiles: *world.resource::<TileOwnership>(),
        }
    }

    /// Overwrites the runner's sim state. Add plugins to the runner separately.
    pub fn restore(self, world: &mut World) {
        world.insert_resource(self.tick);
        world.insert_resource(self.rng);
        world.insert_resource(self.speed);
        world.insert_resource(self.queue);
        world.insert_resource(self.log);
        world.insert_resource(self.tiles);
    }
}

/// Parsed file header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub format_version: u32,
    pub content_hash: u64,
    pub tick: u64,
    pub map_id: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveError {
    BadMagic,
    Truncated,
    /// File is from a newer build than this one.
    UnsupportedVersion(u32),
    Compression(String),
    Decode(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a CPSV save file"),
            Self::Truncated => write!(f, "save file is truncated"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported save format version {v}"),
            Self::Compression(e) => write!(f, "decompression failed: {e}"),
            Self::Decode(e) => write!(f, "save body is corrupt: {e}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Non-fatal findings from [`load`], returned to the caller to show to the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadWarning {
    /// The save was made with different content (defs) than what is loaded now.
    ContentHashMismatch { saved: u64, current: u64 },
}

#[derive(Debug)]
pub struct Loaded {
    pub header: Header,
    pub data: SaveData,
    pub warnings: Vec<LoadWarning>,
}

pub(crate) fn write_header(out: &mut Vec<u8>, h: &Header) {
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&h.format_version.to_le_bytes());
    out.extend_from_slice(&h.content_hash.to_le_bytes());
    out.extend_from_slice(&h.tick.to_le_bytes());
    let id = h.map_id.as_bytes();
    let len = u16::try_from(id.len()).expect("map_id longer than 65535 bytes");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(id);
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8], SaveError> {
    if bytes.len() < n {
        return Err(SaveError::Truncated);
    }
    let (head, tail) = bytes.split_at(n);
    *bytes = tail;
    Ok(head)
}

/// Parses the header, returning it and the compressed body.
pub fn read_header(mut bytes: &[u8]) -> Result<(Header, &[u8]), SaveError> {
    if take(&mut bytes, 4)? != MAGIC {
        return Err(SaveError::BadMagic);
    }
    let format_version = u32::from_le_bytes(take(&mut bytes, 4)?.try_into().unwrap());
    let content_hash = u64::from_le_bytes(take(&mut bytes, 8)?.try_into().unwrap());
    let tick = u64::from_le_bytes(take(&mut bytes, 8)?.try_into().unwrap());
    let len = u16::from_le_bytes(take(&mut bytes, 2)?.try_into().unwrap());
    let map_id = String::from_utf8(take(&mut bytes, usize::from(len))?.to_vec())
        .map_err(|e| SaveError::Decode(e.to_string()))?;
    Ok((
        Header {
            format_version,
            content_hash,
            tick,
            map_id,
        },
        bytes,
    ))
}

pub(crate) fn compress(body: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(body, ruzstd::encoding::CompressionLevel::Fastest)
}

fn decompress(frame: &[u8]) -> Result<Vec<u8>, SaveError> {
    let mut dec = ruzstd::decoding::StreamingDecoder::new(frame)
        .map_err(|e| SaveError::Compression(e.to_string()))?;
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|e| SaveError::Compression(e.to_string()))?;
    Ok(out)
}

/// Encodes `data` as a save file at [`FORMAT_VERSION`].
pub fn save(data: &SaveData, content_hash: u64, map_id: &str) -> Vec<u8> {
    let mut out = Vec::new();
    write_header(
        &mut out,
        &Header {
            format_version: FORMAT_VERSION,
            content_hash,
            tick: data.tick.0,
            map_id: map_id.to_owned(),
        },
    );
    let body = postcard::to_stdvec(data).expect("save serialization failed");
    out.extend_from_slice(&compress(&body));
    out
}

/// Decodes a save file of any known version, migrating it to the current one.
/// `current_content_hash` is the hash of the content now loaded; a difference yields a
/// [`LoadWarning`], not an error.
pub fn load(bytes: &[u8], current_content_hash: u64) -> Result<Loaded, SaveError> {
    let (header, frame) = read_header(bytes)?;
    if header.format_version > FORMAT_VERSION {
        return Err(SaveError::UnsupportedVersion(header.format_version));
    }
    let body = decompress(frame)?;
    let data = migrate::decode_and_migrate(header.format_version, &body)?;
    let mut warnings = Vec::new();
    if header.content_hash != current_content_hash {
        warnings.push(LoadWarning::ContentHashMismatch {
            saved: header.content_hash,
            current: current_content_hash,
        });
    }
    Ok(Loaded {
        header,
        data,
        warnings,
    })
}

/// Saves a runner's state.
pub fn save_runner(sim: &SimRunner, content_hash: u64, map_id: &str) -> Vec<u8> {
    save(&SaveData::capture(sim.world()), content_hash, map_id)
}

/// Loads `bytes` into `sim` (overwriting its sim state) and returns the header and
/// warnings.
pub fn load_into_runner(
    sim: &mut SimRunner,
    bytes: &[u8],
    current_content_hash: u64,
) -> Result<(Header, Vec<LoadWarning>), SaveError> {
    let loaded = load(bytes, current_content_hash)?;
    loaded.data.restore(sim.world_mut());
    Ok((loaded.header, loaded.warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_core::{CommandId, SimSpeed};
    use cp_sim::run_headless;
    use std::path::PathBuf;

    const HASH: u64 = 0xABCD;

    fn fixture(name: &str) -> Vec<u8> {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name);
        std::fs::read(&p).unwrap_or_else(|e| panic!("fixture {p:?}: {e}"))
    }

    /// A sim that has run a while with queued and applied commands.
    fn busy_runner(seed: u64, ticks: u64) -> SimRunner {
        let mut sim = SimRunner::new(seed);
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X3,
            },
            Tick(5),
        );
        sim.push_command(
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
            Tick(10_000),
        );
        for _ in 0..ticks {
            sim.tick_once();
        }
        sim
    }

    #[test]
    fn save_load_save_is_byte_identical() {
        let sim = busy_runner(3, 50);
        let a = save_runner(&sim, HASH, "flatlands");
        assert_eq!(a, save_runner(&sim, HASH, "flatlands"));

        let loaded = load(&a, HASH).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.header.format_version, FORMAT_VERSION);
        assert_eq!(loaded.header.map_id, "flatlands");
        assert_eq!(loaded.header.tick, 50);
        let b = save(&loaded.data, HASH, "flatlands");
        assert_eq!(a, b);
        assert_eq!(loaded.data, SaveData::capture(sim.world()));
        assert_eq!(loaded.data.speed, SimSpeed::X3);
        assert_eq!(loaded.data.queue.len(), 1);
        assert_eq!(loaded.data.log.len(), 1);
    }

    #[test]
    fn load_then_run_matches_a_fresh_run() {
        let (a, n) = (100, 150);
        let mut first = busy_runner(9, a);
        let bytes = save_runner(&first, HASH, "m");

        let mut resumed = SimRunner::new(0); // seed is overwritten by the save
        load_into_runner(&mut resumed, &bytes, HASH).unwrap();
        for _ in 0..n {
            resumed.tick_once();
            first.tick_once();
        }
        assert_eq!(resumed.checksum(), first.checksum());

        // And against run_headless doing the same commands over a + n ticks.
        let cmds = [
            (
                Tick(5),
                SimCommand::SetSpeed {
                    speed: SimSpeed::X3,
                },
            ),
            (
                Tick(10_000),
                SimCommand::SetSpeed {
                    speed: SimSpeed::X2,
                },
            ),
        ];
        assert_eq!(resumed.checksum(), run_headless(9, &cmds, a + n));
    }

    #[test]
    fn content_hash_mismatch_warns_but_loads() {
        let bytes = save_runner(&busy_runner(1, 3), HASH, "m");
        let loaded = load(&bytes, HASH + 1).unwrap();
        assert_eq!(
            loaded.warnings,
            vec![LoadWarning::ContentHashMismatch {
                saved: HASH,
                current: HASH + 1
            }]
        );
        assert_eq!(loaded.data.tick, Tick(3));
    }

    #[test]
    fn v0_fixture_migrates_and_loads() {
        let bytes = fixture("v0-basic.sav");
        let (header, _) = read_header(&bytes).unwrap();
        assert_eq!(header.format_version, 0);

        let loaded = load(&bytes, 0xC17F_0000_0000_0001).unwrap();
        assert!(loaded.warnings.is_empty());
        let d = &loaded.data;
        assert_eq!(d.tick, Tick(300));
        assert_eq!(d.speed, SimSpeed::X1); // added by the v0 -> v1 migration
        assert_eq!(d.tiles, TileOwnership::m1_default());
        assert_eq!(d.queue.len(), 1);
        assert_eq!(d.log.len(), 1);
        assert_eq!(d.log.entries()[0].id, CommandId(0));
        let mut expect = SimRng::seed_from_u64(7);
        for _ in 0..10 {
            expect.next_u64();
        }
        assert_eq!(d.rng, expect);

        // Re-saving writes the current version, which round-trips.
        let resaved = save(d, header.content_hash, &header.map_id);
        assert_eq!(
            read_header(&resaved).unwrap().0.format_version,
            FORMAT_VERSION
        );
        assert_eq!(&load(&resaved, header.content_hash).unwrap().data, d);

        // It also runs: restore into a runner and step.
        let mut sim = SimRunner::new(0);
        d.clone().restore(sim.world_mut());
        sim.tick_once();
        assert_eq!(sim.tick(), Tick(301));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(load(b"NOPE", 0).unwrap_err(), SaveError::BadMagic);
        assert_eq!(load(b"CPS", 0).unwrap_err(), SaveError::Truncated);
        let mut bytes = save_runner(&busy_runner(1, 1), HASH, "m");
        bytes[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert_eq!(
            load(&bytes, HASH).unwrap_err(),
            SaveError::UnsupportedVersion(FORMAT_VERSION + 1)
        );
        let mut bytes = save_runner(&busy_runner(1, 1), HASH, "m");
        let n = bytes.len();
        bytes.truncate(n - 5);
        assert!(load(&bytes, HASH).is_err());
    }
}
