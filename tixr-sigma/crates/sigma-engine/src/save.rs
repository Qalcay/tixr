//! Save files: container format, tamper evidence, and a tiny serialiser.
//!
//! A save slot is a directory with two files that are deliberately treated
//! differently:
//!
//! | file          | holds                                   | protected? |
//! |---------------|-----------------------------------------|------------|
//! | `state.tixr`  | everything the simulation reads on load | yes        |
//! | `history.bin` | chart history (display only)            | no         |
//!
//! # What "protected" means here
//! `state.tixr` is *masked* and *sealed* with keys derived from the world
//! seed and the per-game seed:
//!
//! ```text
//!   key material = SipHash(world seed, game seed, fixed labels)
//!   body         = payload XOR keystream(key)       <- "fuzzing"
//!   tag          = 128-bit keyed SipHash over header + body
//! ```
//!
//! Opening the file in a hex editor shows noise instead of a cash balance,
//! and changing any byte (header seeds included) breaks the tag, so the
//! game refuses to load it. This stops casual editing, which is the goal.
//! It is **not** cryptographic security: the game is offline and its source
//! is public, so someone willing to re-run this exact algorithm can forge a
//! save. No offline single-player game can do better without a server.
//!
//! `history.bin` only has a checksum against disk corruption. Nothing in
//! the simulation reads chart history, so editing it changes pictures, not
//! the game. If it is missing, damaged or from another save, the game
//! still loads and charts simply start empty.
//!
//! # The serialiser
//! No serde (the engine has zero dependencies). [`Persist`] is a two-method
//! trait and the [`record!`](crate::record) macro implements it for a struct
//! from a single field list. Because the macro builds the value with a
//! struct literal, *forgetting a field is a compile error*: the save format
//! cannot silently fall out of step with the structs.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::rng::Rng;

pub const STATE_FILE: &str = "state.tixr";
pub const HISTORY_FILE: &str = "history.bin";
const LOCK_FILE: &str = ".lock";
const STATE_MAGIC: &[u8; 8] = b"TIXRSAV\x01";
const HISTORY_MAGIC: &[u8; 8] = b"TIXRHIS\x01";
/// Bump when the payload layout changes; older files are then rejected
/// with a clear message instead of being misread.
pub const SAVE_VERSION: u32 = 1;
/// Save was produced by a build with time controls that were actually used.
pub const FLAG_DEV_TIME: u32 = 1;
const HEADER_LEN: usize = 8 + 4 + 4 + 8 + 8 + 8 + 8;
const TAG_LEN: usize = 16;
// Constants mixed into the keys so they are specific to this game.
const MASK_WORLD: u64 = 0x5449_5852_5F57_4C44; // "TIXR_WLD"
const MASK_GAME: u64 = 0x5449_5852_5F47_414D; // "TIXR_GAM"
/// A lock older than this is assumed to be left over from a crash.
const STALE_LOCK: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    Io(String),
    /// Not a Tixr-sigma file of the expected kind.
    BadMagic,
    Version(u32),
    /// The seal does not match: the file was edited or damaged.
    Tampered,
    Corrupt(&'static str),
    /// Another running game is saving or loading this slot.
    Locked,
    BadSlotName,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "file error: {e}"),
            SaveError::BadMagic => write!(f, "not a Tixr-sigma save file"),
            SaveError::Version(v) => {
                write!(f, "save format version {v} is not supported (this build reads {SAVE_VERSION})")
            }
            SaveError::Tampered => write!(f, "the save file has been modified or damaged and was not loaded"),
            SaveError::Corrupt(what) => write!(f, "save data is inconsistent ({what})"),
            SaveError::Locked => write!(f, "the slot is in use by another running game"),
            SaveError::BadSlotName => write!(f, "slot names may use letters, digits, '-' and '_' (1 to 32 characters)"),
        }
    }
}

impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, SaveError>;

// ------------------------------------------------------------------ hashing

/// SipHash-2-4 (Aumasson & Bernstein): a small, fast *keyed* hash. Written
/// out here because `std`'s `DefaultHasher` is explicitly not guaranteed to
/// give the same output across Rust releases, which a file format needs.
pub fn siphash24(k0: u64, k1: u64, data: &[u8]) -> u64 {
    #[inline]
    fn round(v: &mut [u64; 4]) {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13);
        v[1] ^= v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16);
        v[3] ^= v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21);
        v[3] ^= v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17);
        v[1] ^= v[2];
        v[2] = v[2].rotate_left(32);
    }
    let mut v = [
        k0 ^ 0x736f_6d65_7073_6575,
        k1 ^ 0x646f_7261_6e64_6f6d,
        k0 ^ 0x6c79_6765_6e65_7261,
        k1 ^ 0x7465_6462_7974_6573,
    ];
    let mut chunks = data.chunks_exact(8);
    for c in &mut chunks {
        let m = u64::from_le_bytes(c.try_into().expect("8 bytes"));
        v[3] ^= m;
        round(&mut v);
        round(&mut v);
        v[0] ^= m;
    }
    let rest = chunks.remainder();
    let mut last = (data.len() as u64) << 56;
    for (i, &b) in rest.iter().enumerate() {
        last |= (b as u64) << (8 * i);
    }
    v[3] ^= last;
    round(&mut v);
    round(&mut v);
    v[0] ^= last;
    v[2] ^= 0xff;
    for _ in 0..4 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

/// A seed nobody chose: wall-clock nanoseconds mixed with the per-process
/// random keys of `std`'s hasher. Used for game seeds and save ids, never
/// for the simulation itself (that would break reproducibility).
pub fn fresh_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(nanos);
    h.write_u32(std::process::id());
    h.finish() ^ nanos.rotate_left(32)
}

struct Keys {
    stream: u64,
    mac: (u64, u64),
}

fn derive_keys(world_seed: u64, game_seed: u64) -> Keys {
    Keys {
        stream: siphash24(world_seed, game_seed, b"tixr-sigma/stream/v1"),
        mac: (
            siphash24(game_seed ^ MASK_WORLD, world_seed.rotate_left(17), b"tixr-sigma/mac-a/v1"),
            siphash24(world_seed ^ MASK_GAME, game_seed.rotate_left(29), b"tixr-sigma/mac-b/v1"),
        ),
    }
}

/// XOR `data` with a keystream. Applying it twice restores the input.
fn mask(data: &mut [u8], stream_key: u64) {
    let mut rng = Rng::seed_from_u64(stream_key);
    for chunk in data.chunks_mut(8) {
        let k = rng.next_u64().to_le_bytes();
        for (b, k) in chunk.iter_mut().zip(k) {
            *b ^= k;
        }
    }
}

fn tag(keys: &Keys, sealed_prefix: &[u8]) -> [u8; TAG_LEN] {
    let mut t = [0u8; TAG_LEN];
    t[..8].copy_from_slice(&siphash24(keys.mac.0, keys.mac.1, sealed_prefix).to_le_bytes());
    t[8..].copy_from_slice(&siphash24(keys.mac.1, keys.mac.0, sealed_prefix).to_le_bytes());
    t
}

/// The clear-text header of a state file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveHeader {
    pub world_seed: u64,
    pub game_seed: u64,
    /// Random id shared with the history sidecar written by the same save.
    pub save_id: u64,
    pub flags: u32,
}

/// Wrap a state payload: header, masked body, seal.
pub fn seal(header: SaveHeader, payload: &[u8]) -> Vec<u8> {
    let keys = derive_keys(header.world_seed, header.game_seed);
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + TAG_LEN);
    out.extend_from_slice(STATE_MAGIC);
    out.extend_from_slice(&SAVE_VERSION.to_le_bytes());
    out.extend_from_slice(&header.flags.to_le_bytes());
    out.extend_from_slice(&(header.world_seed ^ MASK_WORLD).to_le_bytes());
    out.extend_from_slice(&(header.game_seed ^ MASK_GAME).to_le_bytes());
    out.extend_from_slice(&header.save_id.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    let body_start = out.len();
    out.extend_from_slice(payload);
    mask(&mut out[body_start..], keys.stream);
    let t = tag(&keys, &out);
    out.extend_from_slice(&t);
    out
}

/// Verify and unwrap a state file. Any modification yields `Tampered`.
pub fn unseal(file: &[u8]) -> Result<(SaveHeader, Vec<u8>)> {
    if file.len() < HEADER_LEN + TAG_LEN || &file[..8] != STATE_MAGIC {
        return Err(SaveError::BadMagic);
    }
    let u32_at = |i: usize| u32::from_le_bytes(file[i..i + 4].try_into().expect("4 bytes"));
    let u64_at = |i: usize| u64::from_le_bytes(file[i..i + 8].try_into().expect("8 bytes"));
    let version = u32_at(8);
    if version != SAVE_VERSION {
        return Err(SaveError::Version(version));
    }
    let header = SaveHeader {
        flags: u32_at(12),
        world_seed: u64_at(16) ^ MASK_WORLD,
        game_seed: u64_at(24) ^ MASK_GAME,
        save_id: u64_at(32),
    };
    let (sealed, stored_tag) = file.split_at(file.len() - TAG_LEN);
    let keys = derive_keys(header.world_seed, header.game_seed);
    if tag(&keys, sealed) != stored_tag || u64_at(40) as usize != sealed.len() - HEADER_LEN {
        return Err(SaveError::Tampered);
    }
    let mut payload = sealed[HEADER_LEN..].to_vec();
    mask(&mut payload, keys.stream);
    Ok((header, payload))
}

/// Wrap chart history: magic, save id, body, corruption checksum.
pub fn wrap_history(save_id: u64, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 32);
    out.extend_from_slice(HISTORY_MAGIC);
    out.extend_from_slice(&save_id.to_le_bytes());
    out.extend_from_slice(body);
    let sum = siphash24(0, 0, &out);
    out.extend_from_slice(&sum.to_le_bytes());
    out
}

/// Returns the body if the file is intact and belongs to `save_id`.
pub fn unwrap_history(file: &[u8], save_id: u64) -> Result<&[u8]> {
    if file.len() < 24 || &file[..8] != HISTORY_MAGIC {
        return Err(SaveError::BadMagic);
    }
    let (data, sum) = file.split_at(file.len() - 8);
    if siphash24(0, 0, data).to_le_bytes() != sum {
        return Err(SaveError::Corrupt("history checksum"));
    }
    if u64::from_le_bytes(data[8..16].try_into().expect("8 bytes")) != save_id {
        return Err(SaveError::Corrupt("history belongs to a different save"));
    }
    Ok(&data[16..])
}

// --------------------------------------------------------------- serialiser

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or(SaveError::Corrupt("truncated"))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Call when decoding should have consumed everything.
    pub fn finish(&self) -> Result<()> {
        if self.remaining() == 0 { Ok(()) } else { Err(SaveError::Corrupt("trailing bytes")) }
    }
}

/// Binary encoding of a value: little-endian, no padding, no field names.
pub trait Persist: Sized {
    fn put(&self, w: &mut Writer);
    fn get(r: &mut Reader) -> Result<Self>;
}

macro_rules! persist_number {
    ($($t:ty),*) => {$(
        impl Persist for $t {
            fn put(&self, w: &mut Writer) {
                w.buf.extend_from_slice(&self.to_le_bytes());
            }
            fn get(r: &mut Reader) -> Result<Self> {
                let bytes = r.take(std::mem::size_of::<$t>())?;
                Ok(<$t>::from_le_bytes(bytes.try_into().expect("sized")))
            }
        }
    )*};
}
// Floats go through their exact bit patterns: a loaded game continues with
// precisely the numbers it was saved with.
persist_number!(u8, u16, u32, u64, i64, f32, f64);

impl Persist for bool {
    fn put(&self, w: &mut Writer) {
        w.buf.push(*self as u8);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        match u8::get(r)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(SaveError::Corrupt("bool")),
        }
    }
}

impl Persist for usize {
    fn put(&self, w: &mut Writer) {
        (*self as u64).put(w);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(u64::get(r)? as usize)
    }
}

impl Persist for String {
    fn put(&self, w: &mut Writer) {
        (self.len() as u32).put(w);
        w.buf.extend_from_slice(self.as_bytes());
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = u32::get(r)? as usize;
        String::from_utf8(r.take(n)?.to_vec()).map_err(|_| SaveError::Corrupt("text"))
    }
}

/// Read a length prefix, refusing lengths the remaining bytes cannot hold
/// (so a damaged length can never trigger a giant allocation).
fn get_len(r: &mut Reader) -> Result<usize> {
    let n = u32::get(r)? as usize;
    if n > r.remaining() { Err(SaveError::Corrupt("length")) } else { Ok(n) }
}

impl<T: Persist> Persist for Vec<T> {
    fn put(&self, w: &mut Writer) {
        (self.len() as u32).put(w);
        for x in self {
            x.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = get_len(r)?;
        (0..n).map(|_| T::get(r)).collect()
    }
}

impl<T: Persist> Persist for VecDeque<T> {
    fn put(&self, w: &mut Writer) {
        (self.len() as u32).put(w);
        for x in self {
            x.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = get_len(r)?;
        (0..n).map(|_| T::get(r)).collect()
    }
}

impl<T: Persist> Persist for Option<T> {
    fn put(&self, w: &mut Writer) {
        self.is_some().put(w);
        if let Some(x) = self {
            x.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(if bool::get(r)? { Some(T::get(r)?) } else { None })
    }
}

impl<T: Persist, const N: usize> Persist for [T; N] {
    fn put(&self, w: &mut Writer) {
        for x in self {
            x.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let v: Vec<T> = (0..N).map(|_| T::get(r)).collect::<Result<_>>()?;
        v.try_into().map_err(|_| SaveError::Corrupt("array"))
    }
}

impl<A: Persist, B: Persist> Persist for (A, B) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok((A::get(r)?, B::get(r)?))
    }
}

impl<K: Persist + Ord, V: Persist> Persist for BTreeMap<K, V> {
    fn put(&self, w: &mut Writer) {
        (self.len() as u32).put(w);
        for (k, v) in self {
            k.put(w);
            v.put(w);
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let n = get_len(r)?;
        (0..n).map(|_| Ok((K::get(r)?, V::get(r)?))).collect()
    }
}

/// Implement [`Persist`] for a struct from one list of its fields.
///
/// ```ignore
/// record!(Position { shares, cost });
/// // A field that is not saved is rebuilt from an expression instead:
/// record!(Broker { id, name, .. } skip { history: History::new(HistoryKind::Equity) });
/// ```
/// Every field must appear in one of the two lists, or the struct literal
/// inside does not compile. That is the point.
#[macro_export]
macro_rules! record {
    ($ty:ty { $($field:ident),* $(,)? } $(skip { $($skip:ident : $default:expr),* $(,)? })?) => {
        impl $crate::save::Persist for $ty {
            fn put(&self, w: &mut $crate::save::Writer) {
                $( $crate::save::Persist::put(&self.$field, w); )*
            }
            fn get(r: &mut $crate::save::Reader) -> $crate::save::Result<Self> {
                Ok(Self {
                    $( $field: $crate::save::Persist::get(r)?, )*
                    $($( $skip: $default, )*)?
                })
            }
        }
    };
}

/// Implement [`Persist`] for a field-less enum as one byte.
#[macro_export]
macro_rules! tag_enum {
    ($ty:ty { $($variant:ident = $n:literal),* $(,)? }) => {
        impl $crate::save::Persist for $ty {
            fn put(&self, w: &mut $crate::save::Writer) {
                let tag: u8 = match self { $( <$ty>::$variant => $n, )* };
                w.buf.push(tag);
            }
            fn get(r: &mut $crate::save::Reader) -> $crate::save::Result<Self> {
                match <u8 as $crate::save::Persist>::get(r)? {
                    $( $n => Ok(<$ty>::$variant), )*
                    _ => Err($crate::save::SaveError::Corrupt(stringify!($ty))),
                }
            }
        }
    };
}

// ------------------------------------------------------------------- files

/// `root/slot`, after checking the slot name cannot escape `root`.
pub fn slot_dir(root: &Path, slot: &str) -> Result<PathBuf> {
    let ok =
        (1..=32).contains(&slot.len()) && slot.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok { Ok(root.join(slot)) } else { Err(SaveError::BadSlotName) }
}

/// Write via a temporary file and rename, so a crash or power cut mid-save
/// leaves the previous save intact instead of a half-written one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Advisory lock on a slot directory, held while saving or loading so two
/// running games cannot interleave writes. Released on drop.
pub struct SlotLock {
    path: PathBuf,
}

impl SlotLock {
    pub fn acquire(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LOCK_FILE);
        for attempt in 0..2 {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = writeln!(f, "pid {}", std::process::id());
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                    let age = fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
                    if age.is_some_and(|a| a < STALE_LOCK) {
                        return Err(SaveError::Locked);
                    }
                    let _ = fs::remove_file(&path); // stale: left by a crash
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(SaveError::Locked)
    }
}

impl Drop for SlotLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// A save slot found on disk (for the Load menu).
#[derive(Clone, Debug, PartialEq)]
pub struct SlotInfo {
    pub name: String,
    pub modified: Option<SystemTime>,
    pub state_bytes: u64,
    pub history_bytes: u64,
}

/// Slots under `root`, newest first. A missing directory is just "none".
pub fn list_slots(root: &Path) -> Vec<SlotInfo> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    let mut slots: Vec<SlotInfo> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let meta = fs::metadata(e.path().join(STATE_FILE)).ok()?;
            Some(SlotInfo {
                name,
                modified: meta.modified().ok(),
                state_bytes: meta.len(),
                history_bytes: fs::metadata(e.path().join(HISTORY_FILE)).map_or(0, |m| m.len()),
            })
        })
        .collect();
    slots.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siphash_matches_reference_vector() {
        // From the SipHash paper: key 00..0f, message 00..0e.
        let k0 = u64::from_le_bytes([0, 1, 2, 3, 4, 5, 6, 7]);
        let k1 = u64::from_le_bytes([8, 9, 10, 11, 12, 13, 14, 15]);
        let msg: Vec<u8> = (0..15).collect();
        assert_eq!(siphash24(k0, k1, &msg), 0xa129_ca61_49be_45e5);
    }

    const HEADER: SaveHeader = SaveHeader { world_seed: 42, game_seed: 7, save_id: 99, flags: FLAG_DEV_TIME };

    #[test]
    fn seal_round_trips_and_hides_the_payload() {
        let payload = b"cash=1000000;cash=1000000;cash=1000000".to_vec();
        let file = seal(HEADER, &payload);
        assert!(!file.windows(4).any(|w| w == b"cash"), "payload must not be readable in the file");
        assert_eq!(unseal(&file).unwrap(), (HEADER, payload));
    }

    #[test]
    fn any_single_byte_change_is_rejected() {
        let file = seal(HEADER, &[5u8; 300]);
        for i in 0..file.len() {
            let mut bad = file.clone();
            bad[i] ^= 0x01;
            assert!(unseal(&bad).is_err(), "edit at byte {i} went unnoticed");
        }
        // Truncation and extension too.
        assert!(unseal(&file[..file.len() - 1]).is_err());
        let mut longer = file.clone();
        longer.push(0);
        assert!(unseal(&longer).is_err());
    }

    #[test]
    fn body_cannot_be_moved_to_another_game() {
        // Same payload, different game seed: the masked bytes differ, so a
        // body copied from one game's save does not verify in another's.
        let a = seal(HEADER, &[1u8; 64]);
        let b = seal(SaveHeader { game_seed: 8, ..HEADER }, &[1u8; 64]);
        assert_ne!(a[HEADER_LEN..], b[HEADER_LEN..]);
        let mut franken = b.clone();
        franken[HEADER_LEN..].copy_from_slice(&a[HEADER_LEN..]);
        assert_eq!(unseal(&franken), Err(SaveError::Tampered));
    }

    #[test]
    fn history_wrapper_detects_damage_and_wrong_owner() {
        let file = wrap_history(5, b"bars");
        assert_eq!(unwrap_history(&file, 5).unwrap(), b"bars");
        assert!(unwrap_history(&file, 6).is_err());
        let mut bad = file.clone();
        bad[17] ^= 1;
        assert!(unwrap_history(&bad, 5).is_err());
    }

    #[derive(Debug, PartialEq)]
    struct Sample {
        a: u32,
        b: f64,
        c: Vec<Option<String>>,
        d: BTreeMap<u16, [f32; 2]>,
        scratch: u8,
    }
    record!(Sample { a, b, c, d } skip { scratch: 0 });

    #[test]
    fn record_macro_round_trips_and_rejects_truncation() {
        let s = Sample {
            a: 7,
            b: -0.0,
            c: vec![None, Some("héllo".into())],
            d: BTreeMap::from([(1, [1.5, 2.5]), (9, [f32::MIN, 0.0])]),
            scratch: 0,
        };
        let mut w = Writer::default();
        s.put(&mut w);
        let mut r = Reader::new(&w.buf);
        assert_eq!(Sample::get(&mut r).unwrap(), s);
        r.finish().unwrap();
        for cut in 0..w.buf.len() {
            assert!(Sample::get(&mut Reader::new(&w.buf[..cut])).is_err());
        }
    }

    #[test]
    fn slot_names_cannot_escape_the_save_directory() {
        let root = Path::new("saves");
        assert!(slot_dir(root, "slot-1_a").is_ok());
        for bad in ["", "../x", "a/b", "a b", ".", "x".repeat(33).as_str()] {
            assert_eq!(slot_dir(root, bad), Err(SaveError::BadSlotName), "{bad:?}");
        }
    }

    #[test]
    fn lock_excludes_a_second_holder_and_releases_on_drop() {
        let dir = std::env::temp_dir().join(format!("tixr-lock-test-{}", fresh_seed()));
        let first = SlotLock::acquire(&dir).unwrap();
        assert_eq!(SlotLock::acquire(&dir).err(), Some(SaveError::Locked));
        drop(first);
        assert!(SlotLock::acquire(&dir).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }
}
