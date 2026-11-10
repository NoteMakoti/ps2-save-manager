//! PS2 memory card filesystem.
//!
//! The image is held entirely in memory (8–128 MB). All mutations go
//! through page writes so the per-page ECC is kept up to date when the
//! image carries it.
//!
//! Layout (standard 8 MB card): page 0 holds the superblock; clusters
//! (2 pages, 1 KB) 8.. hold the indirect FAT, then the FAT, then the
//! allocatable area starting at `alloc_offset`. Cluster numbers stored
//! in the FAT and in directory entries are relative to `alloc_offset`.

use crate::dirent::{valid_name, DirEntry, Mode, DIRENT_SIZE};
use crate::ecc;
use crate::error::{Error, Result};
use crate::save::{Save, SaveFile};
use crate::tod::Tod;

pub const MAGIC: &[u8; 28] = b"Sony PS2 Memory Card Format ";
pub const FAT_ALLOCATED: u32 = 0x8000_0000;
pub const FAT_CHAIN_END: u32 = 0xFFFF_FFFF;
pub const FAT_FREE: u32 = 0x7FFF_FFFF;
const FAT_MASK: u32 = 0x7FFF_FFFF;
const STANDARD_PAGE_SIZE: usize = 512;
const STANDARD_PAGES_PER_ERASE_BLOCK: usize = 16;
const CLUSTER_SIZE: usize = 1024;
const INDIRECT_FAT_OFFSET: usize = 0x2000;
const MAX_IFC: usize = 32;

/// Size of a standard 8 MB card image with ECC (the `.ps2` / `.mc2` size).
pub const STANDARD_SIZE_ECC: usize = 8_650_752;
/// Size of a standard 8 MB card image without ECC.
pub const STANDARD_SIZE_RAW: usize = 8_388_608;

#[derive(Debug, Clone)]
struct Superblock {
    version: [u8; 12],
    page_size: u16,
    pages_per_cluster: u16,
    pages_per_erase_block: u16,
    unknown: u16,
    clusters_per_card: u32,
    alloc_offset: u32,
    alloc_end: u32,
    rootdir_cluster: u32,
    backup_block1: u32,
    backup_block2: u32,
    ifc_list: [u32; 32],
    bad_block_list: [u32; 32],
    card_type: u8,
    card_flags: u8,
}

impl Superblock {
    fn parse(b: &[u8]) -> Result<Superblock> {
        if b.len() < 0x154 || &b[..28] != MAGIC {
            return Err(Error::NotACard("missing superblock magic".into()));
        }
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let mut version = [0u8; 12];
        version.copy_from_slice(&b[28..40]);
        let mut ifc_list = [0u32; 32];
        let mut bad_block_list = [0u32; 32];
        for i in 0..32 {
            ifc_list[i] = u32_at(0x50 + i * 4);
            bad_block_list[i] = u32_at(0xD0 + i * 4);
        }
        Ok(Superblock {
            version,
            page_size: u16_at(40),
            pages_per_cluster: u16_at(42),
            pages_per_erase_block: u16_at(44),
            unknown: u16_at(46),
            clusters_per_card: u32_at(48),
            alloc_offset: u32_at(52),
            alloc_end: u32_at(56),
            rootdir_cluster: u32_at(60),
            backup_block1: u32_at(64),
            backup_block2: u32_at(68),
            ifc_list,
            bad_block_list,
            card_type: b[0x150],
            card_flags: b[0x151],
        })
    }

    fn to_bytes(&self, page_size: usize) -> Vec<u8> {
        let mut b = vec![0u8; page_size];
        b[..28].copy_from_slice(MAGIC);
        b[28..40].copy_from_slice(&self.version);
        b[40..42].copy_from_slice(&self.page_size.to_le_bytes());
        b[42..44].copy_from_slice(&self.pages_per_cluster.to_le_bytes());
        b[44..46].copy_from_slice(&self.pages_per_erase_block.to_le_bytes());
        b[46..48].copy_from_slice(&self.unknown.to_le_bytes());
        b[48..52].copy_from_slice(&self.clusters_per_card.to_le_bytes());
        b[52..56].copy_from_slice(&self.alloc_offset.to_le_bytes());
        b[56..60].copy_from_slice(&self.alloc_end.to_le_bytes());
        b[60..64].copy_from_slice(&self.rootdir_cluster.to_le_bytes());
        b[64..68].copy_from_slice(&self.backup_block1.to_le_bytes());
        b[68..72].copy_from_slice(&self.backup_block2.to_le_bytes());
        for i in 0..32 {
            b[0x50 + i * 4..0x54 + i * 4].copy_from_slice(&self.ifc_list[i].to_le_bytes());
            b[0xD0 + i * 4..0xD4 + i * 4].copy_from_slice(&self.bad_block_list[i].to_le_bytes());
        }
        b[0x150] = self.card_type;
        b[0x151] = self.card_flags;
        b
    }
}

/// Location of a directory entry: first cluster of the directory that
/// contains it, and its index there. The root directory is `(root, 0)`
/// — its own "." entry, which also carries the root's entry count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Loc {
    dir: u32,
    index: u32,
}

/// Summary information about a card image.
#[derive(Debug, Clone)]
pub struct CardInfo {
    pub image_size: usize,
    pub has_ecc: bool,
    pub page_size: usize,
    pub cluster_size: usize,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub version: String,
}

/// Summary of one top-level save directory.
#[derive(Debug, Clone)]
pub struct SaveSummary {
    pub name: String,
    pub title: String,
    pub size: u64,
    pub files: usize,
    pub created: Tod,
    pub modified: Tod,
    pub mode: Mode,
}

pub struct MemoryCard {
    data: Vec<u8>,
    sb: Superblock,
    ecc: bool,
    page_size: usize,
    raw_page: usize,
    ppc: usize,
    cluster_size: usize,
    epc: usize,
    alloc_cursor: u32,
    dirty: bool,
}

impl std::fmt::Debug for MemoryCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryCard")
            .field("size", &self.data.len())
            .field("ecc", &self.ecc)
            .finish()
    }
}

fn split_path(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|s| !s.is_empty() && *s != ".").collect()
}

impl MemoryCard {
    // ------------------------------------------------------------------
    // Opening / creating
    // ------------------------------------------------------------------

    /// Open a card image from its raw bytes (with or without ECC).
    pub fn open(data: Vec<u8>) -> Result<MemoryCard> {
        if data.len() < 0x154 {
            return Err(Error::NotACard("file too small".into()));
        }
        let sb = Superblock::parse(&data[..0x154])?;
        let page_size = sb.page_size as usize;
        let ppc = sb.pages_per_cluster as usize;
        if page_size == 0 || page_size % 128 != 0 || ppc == 0 || page_size * ppc > 64 * 1024 {
            return Err(Error::Corrupt(format!(
                "bad geometry: page {page_size}, pages/cluster {ppc}"
            )));
        }
        let pages = sb.clusters_per_card as usize * ppc;
        let spare = page_size / 32;
        let ecc = if data.len() >= pages * (page_size + spare) {
            true
        } else if data.len() >= pages * page_size {
            false
        } else {
            return Err(Error::Corrupt(format!(
                "image is {} bytes, expected at least {}",
                data.len(),
                pages * page_size
            )));
        };
        let cluster_size = page_size * ppc;
        let card = MemoryCard {
            data,
            ecc,
            page_size,
            raw_page: if ecc { page_size + spare } else { page_size },
            ppc,
            cluster_size,
            epc: cluster_size / 4,
            alloc_cursor: 0,
            dirty: false,
            sb,
        };
        let dot = card.entry_at(card.root_loc())?;
        if dot.name != "." || !dot.mode.is_dir() {
            return Err(Error::Corrupt("root directory damaged".into()));
        }
        Ok(card)
    }

    /// Read and open an image file.
    pub fn open_file(path: impl AsRef<std::path::Path>) -> Result<MemoryCard> {
        MemoryCard::open(std::fs::read(path)?)
    }

    /// Create a freshly formatted card. `size_mb` is the nominal capacity
    /// (8, 16, 32, 64 or 128); `with_ecc` selects the 528-byte page layout
    /// used by PCSX2 `.ps2` files and the MemCard PRO `.mc2` files.
    pub fn format(size_mb: usize, with_ecc: bool) -> Result<MemoryCard> {
        if !size_mb.is_power_of_two() || !(1..=2048).contains(&size_mb) {
            return Err(Error::Invalid(format!("unsupported card size {size_mb} MB")));
        }
        let page_size = STANDARD_PAGE_SIZE;
        let ppeb = STANDARD_PAGES_PER_ERASE_BLOCK;
        let pages_per_card = size_mb * 1024 * 1024 / page_size;
        let ppc = CLUSTER_SIZE / page_size;
        let clusters_per_eb = ppeb / ppc;
        let erase_blocks = pages_per_card / ppeb;
        let clusters_per_card = pages_per_card / ppc;
        let epc = CLUSTER_SIZE / 4;

        let good_block1 = erase_blocks - 1;
        let good_block2 = erase_blocks - 2;
        let first_ifc = INDIRECT_FAT_OFFSET.div_ceil(CLUSTER_SIZE);
        let mut allocatable = clusters_per_card - (first_ifc + 2);
        let mut fat_clusters = allocatable.div_ceil(epc);
        let mut ifc_count = fat_clusters.div_ceil(epc);
        if ifc_count > MAX_IFC {
            ifc_count = MAX_IFC;
            fat_clusters = ifc_count * epc;
        }
        allocatable = fat_clusters * epc;
        let alloc_offset = first_ifc + ifc_count + fat_clusters;
        let alloc_end = good_block2 * clusters_per_eb - alloc_offset;

        let mut ifc_list = [0u32; 32];
        for (i, slot) in ifc_list.iter_mut().enumerate().take(ifc_count) {
            *slot = (first_ifc + i) as u32;
        }
        let mut version = [0u8; 12];
        version[..7].copy_from_slice(b"1.2.0.0");
        let sb = Superblock {
            version,
            page_size: page_size as u16,
            pages_per_cluster: ppc as u16,
            pages_per_erase_block: ppeb as u16,
            unknown: 0xFF00,
            clusters_per_card: clusters_per_card as u32,
            alloc_offset: alloc_offset as u32,
            alloc_end: alloc_end as u32,
            rootdir_cluster: 0,
            backup_block1: good_block1 as u32,
            backup_block2: good_block2 as u32,
            ifc_list,
            bad_block_list: [0xFFFF_FFFF; 32],
            card_type: 2,
            card_flags: 0x52,
        };

        let spare = page_size / 32;
        let raw_page = if with_ecc { page_size + spare } else { page_size };
        let mut erased = vec![0u8; raw_page];
        if with_ecc {
            let (p, s) = erased.split_at_mut(page_size);
            ecc::ecc_page(p, s);
        }
        let mut data = Vec::with_capacity(pages_per_card * raw_page);
        for _ in 0..pages_per_card {
            data.extend_from_slice(&erased);
        }

        let mut card = MemoryCard {
            data,
            ecc: with_ecc,
            page_size,
            raw_page,
            ppc,
            cluster_size: CLUSTER_SIZE,
            epc,
            alloc_cursor: 0,
            dirty: true,
            sb,
        };

        // Indirect FAT clusters point at the FAT clusters.
        let first_fat = first_ifc + ifc_count;
        for i in 0..ifc_count {
            let mut buf = vec![0u8; CLUSTER_SIZE];
            for j in 0..epc {
                let fat_index = i * epc + j;
                let v = if fat_index < fat_clusters {
                    (first_fat + fat_index) as u32
                } else {
                    0xFFFF_FFFF
                };
                buf[j * 4..j * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            card.write_cluster(card.sb.ifc_list[i] as usize, &buf);
        }
        // FAT: free inside the allocatable area, "end" beyond it.
        for f in 0..fat_clusters {
            let mut buf = vec![0u8; CLUSTER_SIZE];
            for j in 0..epc {
                let n = f * epc + j;
                let v = if n == 0 || n >= alloc_end && n < allocatable {
                    FAT_CHAIN_END
                } else if n < alloc_end {
                    FAT_FREE
                } else {
                    FAT_CHAIN_END
                };
                buf[j * 4..j * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            card.write_cluster(first_fat + f, &buf);
        }

        // Root directory: "." and ".." in allocatable cluster 0.
        let now = Tod::now();
        let mut root = vec![0u8; CLUSTER_SIZE];
        let mut dot = DirEntry::new(".", Mode::save_dir(), 2, 0, now);
        dot.dir_entry = 0;
        let dotdot = DirEntry::new(
            "..",
            Mode(Mode::WRITE | Mode::EXECUTE | Mode::DIR | Mode::F_0400 | Mode::HIDDEN | Mode::EXISTS),
            0,
            0,
            now,
        );
        root[..DIRENT_SIZE].copy_from_slice(&dot.to_bytes());
        root[DIRENT_SIZE..2 * DIRENT_SIZE].copy_from_slice(&dotdot.to_bytes());
        card.write_cluster(alloc_offset, &root);

        // Superblock, and the erased backup block.
        let sbb = card.sb.to_bytes(page_size);
        card.write_page(0, &sbb);
        let start = good_block2 * ppeb * raw_page;
        card.data[start..start + ppeb * raw_page].fill(0xFF);
        Ok(card)
    }

    // ------------------------------------------------------------------
    // Raw access
    // ------------------------------------------------------------------

    /// The raw image bytes (in the original ECC / no-ECC layout).
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }

    /// Image bytes converted to the requested layout.
    pub fn to_bytes_with_ecc(&self, with_ecc: bool) -> Vec<u8> {
        if with_ecc == self.ecc {
            return self.data.clone();
        }
        let pages = self.sb.clusters_per_card as usize * self.ppc;
        let spare = self.page_size / 32;
        let mut out = Vec::with_capacity(pages * (self.page_size + if with_ecc { spare } else { 0 }));
        let mut sp = vec![0u8; spare];
        for p in 0..pages {
            let page = self.page(p);
            out.extend_from_slice(page);
            if with_ecc {
                ecc::ecc_page(page, &mut sp);
                out.extend_from_slice(&sp);
            }
        }
        out
    }

    pub fn has_ecc(&self) -> bool {
        self.ecc
    }

    /// True if anything was changed since the image was opened.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn save_to(&mut self, path: impl AsRef<std::path::Path>) -> Result<()> {
        let path = path.as_ref();
        let tmp = path.with_extension("tmp~");
        std::fs::write(&tmp, &self.data)?;
        std::fs::rename(&tmp, path)?;
        self.dirty = false;
        Ok(())
    }

    fn page(&self, n: usize) -> &[u8] {
        let o = n * self.raw_page;
        &self.data[o..o + self.page_size]
    }

    fn write_page(&mut self, n: usize, buf: &[u8]) {
        let o = n * self.raw_page;
        self.data[o..o + self.page_size].copy_from_slice(&buf[..self.page_size]);
        if self.ecc {
            let (p, s) = self.data[o..o + self.raw_page].split_at_mut(self.page_size);
            ecc::ecc_page(p, s);
        }
        self.dirty = true;
    }

    fn read_cluster(&self, abs: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.cluster_size);
        for p in 0..self.ppc {
            out.extend_from_slice(self.page(abs * self.ppc + p));
        }
        out
    }

    fn write_cluster(&mut self, abs: usize, buf: &[u8]) {
        for p in 0..self.ppc {
            let s = p * self.page_size;
            self.write_page(abs * self.ppc + p, &buf[s..s + self.page_size]);
        }
    }

    fn read_u32(&self, abs_cluster: usize, idx: usize) -> u32 {
        let byte = idx * 4;
        let page = self.page(abs_cluster * self.ppc + byte / self.page_size);
        let o = byte % self.page_size;
        u32::from_le_bytes([page[o], page[o + 1], page[o + 2], page[o + 3]])
    }

    fn write_u32(&mut self, abs_cluster: usize, idx: usize, v: u32) {
        let byte = idx * 4;
        let pn = abs_cluster * self.ppc + byte / self.page_size;
        let mut page = self.page(pn).to_vec();
        let o = byte % self.page_size;
        page[o..o + 4].copy_from_slice(&v.to_le_bytes());
        self.write_page(pn, &page);
    }

    // ------------------------------------------------------------------
    // FAT
    // ------------------------------------------------------------------

    fn fat_loc(&self, n: u32) -> Result<(usize, usize)> {
        let n = n as usize;
        let fat_offset = n % self.epc;
        let fat_index = n / self.epc;
        let ifc_index = fat_index / self.epc;
        let ifc_offset = fat_index % self.epc;
        if ifc_index >= MAX_IFC {
            return Err(Error::Corrupt(format!("cluster {n} outside FAT")));
        }
        let ifc = self.sb.ifc_list[ifc_index] as usize;
        let fat_cluster = self.read_u32(ifc, ifc_offset) as usize;
        if fat_cluster >= self.sb.clusters_per_card as usize {
            return Err(Error::Corrupt(format!("bad FAT cluster {fat_cluster:#x}")));
        }
        Ok((fat_cluster, fat_offset))
    }

    fn get_fat(&self, n: u32) -> Result<u32> {
        let (c, o) = self.fat_loc(n)?;
        Ok(self.read_u32(c, o))
    }

    fn set_fat(&mut self, n: u32, v: u32) -> Result<()> {
        let (c, o) = self.fat_loc(n)?;
        self.write_u32(c, o, v);
        Ok(())
    }

    /// Follow a cluster chain. Returns relative cluster numbers.
    #[allow(dead_code)]
    fn chain(&self, first: u32) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut c = first;
        while c != FAT_CHAIN_END {
            if c >= self.sb.alloc_end {
                return Err(Error::Corrupt(format!("cluster {c} beyond end of card")));
            }
            out.push(c);
            if out.len() > self.sb.alloc_end as usize {
                return Err(Error::Corrupt("cluster chain loops".into()));
            }
            let v = self.get_fat(c)?;
            if v & FAT_ALLOCATED == 0 {
                return Err(Error::Corrupt(format!("chain runs into free cluster {c}")));
            }
            c = if v == FAT_CHAIN_END { FAT_CHAIN_END } else { v & FAT_MASK };
        }
        Ok(out)
    }

    /// Like `chain` but stops after `count` clusters.
    fn chain_n(&self, first: u32, count: usize) -> Result<Vec<u32>> {
        let mut out = Vec::with_capacity(count);
        let mut c = first;
        while out.len() < count {
            if c == FAT_CHAIN_END || c >= self.sb.alloc_end {
                return Err(Error::Corrupt(format!("cluster chain too short ({} of {count})", out.len())));
            }
            out.push(c);
            let v = self.get_fat(c)?;
            if v & FAT_ALLOCATED == 0 {
                return Err(Error::Corrupt(format!("chain runs into free cluster {c}")));
            }
            c = if v == FAT_CHAIN_END { FAT_CHAIN_END } else { v & FAT_MASK };
        }
        Ok(out)
    }

    fn allocate(&mut self) -> Result<u32> {
        let end = self.sb.alloc_end;
        for pass in 0..2 {
            let (from, to) = if pass == 0 { (self.alloc_cursor, end) } else { (0, self.alloc_cursor) };
            for n in from..to {
                let v = self.get_fat(n)?;
                if v & FAT_ALLOCATED == 0 {
                    self.set_fat(n, FAT_CHAIN_END)?;
                    self.alloc_cursor = n + 1;
                    return Ok(n);
                }
            }
        }
        Err(Error::NoSpace { needed: 1, free: 0 })
    }

    /// Allocate `count` clusters as a linked chain; returns them in order.
    fn allocate_chain(&mut self, count: usize) -> Result<Vec<u32>> {
        let free = self.free_clusters()?;
        if free < count {
            return Err(Error::NoSpace {
                needed: (count * self.cluster_size / 1024) as u64,
                free: (free * self.cluster_size / 1024) as u64,
            });
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let c = self.allocate()?;
            if let Some(&prev) = out.last() {
                self.set_fat(prev, FAT_ALLOCATED | c)?;
            }
            out.push(c);
        }
        Ok(out)
    }

    fn free_chain(&mut self, first: u32) -> Result<()> {
        let mut c = first;
        let mut guard = 0;
        while c != FAT_CHAIN_END && c < self.sb.alloc_end {
            let v = self.get_fat(c)?;
            if v & FAT_ALLOCATED == 0 {
                break;
            }
            let next = v & FAT_MASK;
            self.set_fat(c, next)?;
            if c < self.alloc_cursor {
                self.alloc_cursor = c;
            }
            if next == FAT_FREE {
                break;
            }
            c = next;
            guard += 1;
            if guard > self.sb.alloc_end {
                return Err(Error::Corrupt("cluster chain loops".into()));
            }
        }
        Ok(())
    }

    pub fn free_clusters(&self) -> Result<usize> {
        let mut n = 0;
        for c in 0..self.sb.alloc_end {
            if self.get_fat(c)? & FAT_ALLOCATED == 0 {
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn cluster_size(&self) -> usize {
        self.cluster_size
    }

    pub fn info(&self) -> Result<CardInfo> {
        let free = self.free_clusters()?;
        Ok(CardInfo {
            image_size: self.data.len(),
            has_ecc: self.ecc,
            page_size: self.page_size,
            cluster_size: self.cluster_size,
            total_bytes: self.sb.alloc_end as u64 * self.cluster_size as u64,
            free_bytes: free as u64 * self.cluster_size as u64,
            version: String::from_utf8_lossy(&self.sb.version)
                .trim_end_matches('\0')
                .to_string(),
        })
    }

    // ------------------------------------------------------------------
    // Directory entries
    // ------------------------------------------------------------------

    fn ents_per_cluster(&self) -> u32 {
        (self.cluster_size / DIRENT_SIZE) as u32
    }

    fn root_loc(&self) -> Loc {
        Loc { dir: self.sb.rootdir_cluster, index: 0 }
    }

    fn entry_cluster(&self, loc: Loc) -> Result<u32> {
        let k = (loc.index / self.ents_per_cluster()) as usize;
        let chain = self.chain_n(loc.dir, k + 1)?;
        Ok(chain[k])
    }

    fn entry_at(&self, loc: Loc) -> Result<DirEntry> {
        let c = self.entry_cluster(loc)?;
        let abs = (c + self.sb.alloc_offset) as usize;
        let off = (loc.index % self.ents_per_cluster()) as usize * DIRENT_SIZE;
        let page_n = abs * self.ppc + off / self.page_size;
        let in_page = off % self.page_size;
        let mut buf = Vec::with_capacity(DIRENT_SIZE);
        // a dirent may span pages if page_size < 512 (it never does on
        // real cards, but be safe)
        let mut p = page_n;
        let mut o = in_page;
        while buf.len() < DIRENT_SIZE {
            let page = self.page(p);
            let take = (self.page_size - o).min(DIRENT_SIZE - buf.len());
            buf.extend_from_slice(&page[o..o + take]);
            p += 1;
            o = 0;
        }
        Ok(DirEntry::parse(&buf))
    }

    fn put_entry(&mut self, loc: Loc, ent: &DirEntry) -> Result<()> {
        let c = self.entry_cluster(loc)?;
        let abs = (c + self.sb.alloc_offset) as usize;
        let mut cl = self.read_cluster(abs);
        let off = (loc.index % self.ents_per_cluster()) as usize * DIRENT_SIZE;
        cl[off..off + DIRENT_SIZE].copy_from_slice(&ent.to_bytes());
        self.write_cluster(abs, &cl);
        Ok(())
    }

    /// All entries (including deleted ones) of the directory whose own
    /// entry is at `loc`.
    fn dir_entries(&self, loc: Loc) -> Result<Vec<(u32, DirEntry)>> {
        let ent = self.entry_at(loc)?;
        if !ent.mode.is_dir() {
            return Err(Error::NotADirectory(ent.name));
        }
        let n = ent.length;
        let per = self.ents_per_cluster();
        let chain = self.chain_n(ent.cluster, n.div_ceil(per) as usize)?;
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let c = chain[(i / per) as usize];
            let abs = (c + self.sb.alloc_offset) as usize;
            let off = (i % per) as usize * DIRENT_SIZE;
            let cl_page = abs * self.ppc + off / self.page_size;
            let o = off % self.page_size;
            let e = if o + DIRENT_SIZE <= self.page_size {
                DirEntry::parse(&self.page(cl_page)[o..o + DIRENT_SIZE])
            } else {
                self.entry_at(Loc { dir: ent.cluster, index: i })?
            };
            out.push((i, e));
        }
        Ok(out)
    }

    fn lookup(&self, path: &str) -> Result<Loc> {
        let mut loc = self.root_loc();
        for comp in split_path(path) {
            if comp == ".." {
                return Err(Error::InvalidName(path.into()));
            }
            let dir = self.entry_at(loc)?;
            if !dir.mode.is_dir() {
                return Err(Error::NotADirectory(path.into()));
            }
            let found = self
                .dir_entries(loc)?
                .into_iter()
                .skip(2)
                .find(|(_, e)| e.mode.exists() && e.name == comp)
                .ok_or_else(|| Error::NotFound(path.into()))?;
            loc = Loc { dir: dir.cluster, index: found.0 };
        }
        Ok(loc)
    }

    fn parent_and_name(path: &str) -> Result<(String, String)> {
        let parts = split_path(path);
        let name = parts.last().ok_or_else(|| Error::InvalidName(path.into()))?.to_string();
        if !valid_name(&name) {
            return Err(Error::InvalidName(name));
        }
        Ok((parts[..parts.len() - 1].join("/"), name))
    }

    pub fn exists(&self, path: &str) -> bool {
        self.lookup(path).is_ok()
    }

    pub fn stat(&self, path: &str) -> Result<DirEntry> {
        let loc = self.lookup(path)?;
        self.entry_at(loc)
    }

    /// List a directory (existing entries, without "." and "..").
    pub fn list(&self, path: &str) -> Result<Vec<DirEntry>> {
        let loc = self.lookup(path)?;
        Ok(self
            .dir_entries(loc)?
            .into_iter()
            .skip(2)
            .map(|(_, e)| e)
            .filter(|e| e.mode.exists())
            .collect())
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let ent = self.stat(path)?;
        if ent.mode.is_dir() {
            return Err(Error::IsADirectory(path.into()));
        }
        self.read_entry_data(&ent)
    }

    fn read_entry_data(&self, ent: &DirEntry) -> Result<Vec<u8>> {
        let len = ent.length as usize;
        if len == 0 {
            return Ok(Vec::new());
        }
        let chain = self.chain_n(ent.cluster, len.div_ceil(self.cluster_size))?;
        let mut out = Vec::with_capacity(chain.len() * self.cluster_size);
        for c in chain {
            out.extend_from_slice(&self.read_cluster((c + self.sb.alloc_offset) as usize));
        }
        out.truncate(len);
        Ok(out)
    }

    /// Make a new entry in the directory at `parent`.
    fn create_entry(&mut self, parent: Loc, name: &str, mode: Mode, created: Tod, modified: Tod) -> Result<Loc> {
        let mut pent = self.entry_at(parent)?;
        let entries = self.dir_entries(parent)?;
        if entries.iter().skip(2).any(|(_, e)| e.mode.exists() && e.name == name) {
            return Err(Error::Exists(name.into()));
        }
        let per = self.ents_per_cluster();
        let free_slot = entries.iter().skip(2).find(|(_, e)| !e.mode.exists()).map(|(i, _)| *i);

        // Space check: a directory needs one cluster for "." / "..".
        let need = (mode.0 & Mode::DIR != 0) as usize
            + (free_slot.is_none() && pent.length % per == 0) as usize;
        let free = self.free_clusters()?;
        if free < need {
            return Err(Error::NoSpace { needed: need as u64, free: free as u64 });
        }

        let index = match free_slot {
            Some(i) => i,
            None => {
                let i = pent.length;
                if i % per == 0 {
                    // directory needs another cluster
                    let chain = self.chain_n(pent.cluster, (i / per) as usize)?;
                    let c = self.allocate()?;
                    self.write_cluster((c + self.sb.alloc_offset) as usize, &vec![0u8; self.cluster_size]);
                    self.set_fat(*chain.last().unwrap(), FAT_ALLOCATED | c)?;
                }
                pent.length = i + 1;
                pent.modified = Tod::now();
                self.put_entry(parent, &pent)?;
                i
            }
        };
        let loc = Loc { dir: pent.cluster, index };
        let mut ent = DirEntry::new(name, mode, 0, FAT_CHAIN_END, created);
        ent.modified = modified;
        if mode.0 & Mode::DIR != 0 {
            ent.mode = Mode((mode.0 & !Mode::FILE) | Mode::DIR | Mode::EXISTS);
            let c = self.allocate()?;
            let mut cl = vec![0u8; self.cluster_size];
            let mut dot = DirEntry::new(".", Mode::save_dir(), 0, pent.cluster, created);
            dot.dir_entry = index;
            let dotdot = DirEntry::new("..", Mode::save_dir(), 0, 0, created);
            cl[..DIRENT_SIZE].copy_from_slice(&dot.to_bytes());
            cl[DIRENT_SIZE..2 * DIRENT_SIZE].copy_from_slice(&dotdot.to_bytes());
            self.write_cluster((c + self.sb.alloc_offset) as usize, &cl);
            ent.cluster = c;
            ent.length = 2;
        } else {
            ent.mode = Mode((mode.0 & !Mode::DIR) | Mode::FILE | Mode::EXISTS);
        }
        self.put_entry(loc, &ent)?;
        Ok(loc)
    }

    /// Create a directory (parent must exist).
    pub fn mkdir(&mut self, path: &str) -> Result<()> {
        let now = Tod::now();
        self.mkdir_with(path, Mode::save_dir(), now, now)
    }

    pub fn mkdir_with(&mut self, path: &str, mode: Mode, created: Tod, modified: Tod) -> Result<()> {
        let (parent, name) = Self::parent_and_name(path)?;
        let ploc = self.lookup(&parent)?;
        self.create_entry(ploc, &name, Mode(mode.0 | Mode::DIR), created, modified)?;
        Ok(())
    }

    /// Create or replace a file.
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()> {
        let now = Tod::now();
        match self.stat(path) {
            Ok(e) => self.write_file_with(path, data, e.mode, e.created, now),
            Err(_) => self.write_file_with(path, data, Mode::save_file(), now, now),
        }
    }

    pub fn write_file_with(&mut self, path: &str, data: &[u8], mode: Mode, created: Tod, modified: Tod) -> Result<()> {
        let (parent, name) = Self::parent_and_name(path)?;
        let ploc = self.lookup(&parent)?;
        let pent = self.entry_at(ploc)?;
        let need = data.len().div_ceil(self.cluster_size);
        let (loc, mut ent) = match self.lookup(path) {
            Ok(loc) => {
                let e = self.entry_at(loc)?;
                if e.mode.is_dir() {
                    return Err(Error::IsADirectory(path.into()));
                }
                // space check counting the clusters we are about to free
                let have = (e.length as usize).div_ceil(self.cluster_size);
                let free = self.free_clusters()? + have;
                if free < need {
                    return Err(Error::NoSpace {
                        needed: (need * self.cluster_size / 1024) as u64,
                        free: (free * self.cluster_size / 1024) as u64,
                    });
                }
                if e.length > 0 {
                    self.free_chain(e.cluster)?;
                }
                (loc, e)
            }
            Err(Error::NotFound(_)) => {
                let free = self.free_clusters()?;
                if free < need + 1 {
                    return Err(Error::NoSpace {
                        needed: (need * self.cluster_size / 1024) as u64,
                        free: (free * self.cluster_size / 1024) as u64,
                    });
                }
                let loc = self.create_entry(ploc, &name, Mode(mode.0 & !Mode::DIR), created, modified)?;
                (loc, self.entry_at(loc)?)
            }
            Err(e) => return Err(e),
        };
        let chain = self.allocate_chain(need)?;
        for (i, c) in chain.iter().enumerate() {
            let s = i * self.cluster_size;
            let e = (s + self.cluster_size).min(data.len());
            let mut buf = vec![0u8; self.cluster_size];
            buf[..e - s].copy_from_slice(&data[s..e]);
            self.write_cluster((*c + self.sb.alloc_offset) as usize, &buf);
        }
        ent.mode = Mode((mode.0 & !Mode::DIR) | Mode::FILE | Mode::EXISTS);
        ent.length = data.len() as u32;
        ent.cluster = chain.first().copied().unwrap_or(FAT_CHAIN_END);
        ent.created = created;
        ent.modified = modified;
        self.put_entry(loc, &ent)?;
        let _ = pent;
        Ok(())
    }

    /// Remove a file or an empty directory.
    pub fn remove(&mut self, path: &str) -> Result<()> {
        let loc = self.lookup(path)?;
        if loc == self.root_loc() || loc.index < 2 {
            return Err(Error::Invalid("cannot remove root, \".\" or \"..\"".into()));
        }
        let mut ent = self.entry_at(loc)?;
        if ent.mode.is_dir() {
            let has_children = self.dir_entries(loc)?.iter().skip(2).any(|(_, e)| e.mode.exists());
            if has_children {
                return Err(Error::NotEmpty(path.into()));
            }
        }
        let first = ent.cluster;
        let had_data = ent.mode.is_dir() || ent.length > 0;
        ent.mode = Mode(ent.mode.0 & !Mode::EXISTS);
        self.put_entry(loc, &ent)?;
        if had_data && first != FAT_CHAIN_END {
            self.free_chain(first)?;
        }
        Ok(())
    }

    /// Remove a directory and everything in it (or a single file).
    pub fn remove_all(&mut self, path: &str) -> Result<()> {
        let ent = self.stat(path)?;
        if ent.mode.is_dir() {
            for child in self.list(path)? {
                let p = format!("{}/{}", path.trim_end_matches('/'), child.name);
                self.remove_all(&p)?;
            }
        }
        self.remove(path)
    }

    /// Rename an entry in place (no moves between directories).
    pub fn rename(&mut self, path: &str, new_name: &str) -> Result<()> {
        if !valid_name(new_name) {
            return Err(Error::InvalidName(new_name.into()));
        }
        let (parent, _) = Self::parent_and_name(path)?;
        if self.exists(&format!("{parent}/{new_name}")) {
            return Err(Error::Exists(new_name.into()));
        }
        let loc = self.lookup(path)?;
        let mut ent = self.entry_at(loc)?;
        ent.name = new_name.to_string();
        self.put_entry(loc, &ent)
    }

    /// Change mode/timestamps of an existing entry.
    pub fn set_entry_info(&mut self, path: &str, mode: Option<Mode>, created: Option<Tod>, modified: Option<Tod>) -> Result<()> {
        let loc = self.lookup(path)?;
        let mut ent = self.entry_at(loc)?;
        if let Some(m) = mode {
            let kind = ent.mode.0 & (Mode::DIR | Mode::FILE | Mode::EXISTS);
            ent.mode = Mode((m.0 & !(Mode::DIR | Mode::FILE)) | kind);
        }
        if let Some(c) = created {
            ent.created = c;
        }
        if let Some(m) = modified {
            ent.modified = m;
        }
        self.put_entry(loc, &ent)
    }

    // ------------------------------------------------------------------
    // Saves
    // ------------------------------------------------------------------

    /// Clusters a save would occupy on this card.
    pub fn clusters_for_save(&self, save: &Save) -> usize {
        let per = self.ents_per_cluster() as usize;
        (save.files.len() + 2).div_ceil(per)
            + save
                .files
                .iter()
                .map(|f| f.data.len().div_ceil(self.cluster_size))
                .sum::<usize>()
    }

    /// One summary per top-level directory.
    pub fn saves(&self) -> Result<Vec<SaveSummary>> {
        let mut out = Vec::new();
        for d in self.list("/")? {
            if !d.mode.is_dir() {
                continue;
            }
            let files = self.list(&d.name).unwrap_or_default();
            let size: u64 = files.iter().map(|f| f.length as u64).sum();
            let title = self
                .read_file(&format!("{}/icon.sys", d.name))
                .ok()
                .and_then(|b| crate::iconsys::IconSys::parse(&b).ok())
                .map(|i| i.title())
                .unwrap_or_default();
            out.push(SaveSummary {
                name: d.name.clone(),
                title,
                size,
                files: files.len(),
                created: d.created,
                modified: d.modified,
                mode: d.mode,
            });
        }
        Ok(out)
    }

    /// Copy a top-level save directory out of the card.
    pub fn export_save(&self, name: &str) -> Result<Save> {
        let dir = self.stat(name)?;
        if !dir.mode.is_dir() {
            return Err(Error::NotADirectory(name.into()));
        }
        let mut files = Vec::new();
        for f in self.list(name)? {
            if f.mode.is_dir() {
                // saves never contain subdirectories; skip defensively
                continue;
            }
            let data = self.read_entry_data(&f)?;
            files.push(SaveFile {
                name: f.name.clone(),
                mode: f.mode,
                created: f.created,
                modified: f.modified,
                data,
            });
        }
        Ok(Save {
            name: dir.name.clone(),
            mode: dir.mode,
            created: dir.created,
            modified: dir.modified,
            files,
        })
    }

    /// Write a save onto the card. If a save with the same name exists it
    /// is replaced when `overwrite` is set, otherwise `Error::Exists`.
    pub fn import_save(&mut self, save: &Save, overwrite: bool) -> Result<()> {
        if !valid_name(&save.name) {
            return Err(Error::InvalidName(save.name.clone()));
        }
        let mut available = self.free_clusters()?;
        if self.exists(&save.name) {
            if !overwrite {
                return Err(Error::Exists(save.name.clone()));
            }
            let old = self.export_save(&save.name)?;
            available += self.clusters_for_save(&old);
        }
        let need = self.clusters_for_save(save)
            + (self.dir_entries(self.root_loc())?.iter().skip(2).all(|(_, e)| e.mode.exists())) as usize;
        if need > available {
            return Err(Error::NoSpace {
                needed: (need * self.cluster_size / 1024) as u64,
                free: (available * self.cluster_size / 1024) as u64,
            });
        }
        if self.exists(&save.name) {
            self.remove_all(&save.name)?;
        }
        let dir_mode = if save.mode.is_dir() { save.mode } else { Mode::save_dir() };
        self.mkdir_with(&save.name, dir_mode, save.created, save.modified)?;
        for f in &save.files {
            if !valid_name(&f.name) {
                return Err(Error::InvalidName(f.name.clone()));
            }
            let mode = if f.mode.is_file() { f.mode } else { Mode::save_file() };
            self.write_file_with(&format!("{}/{}", save.name, f.name), &f.data, mode, f.created, f.modified)?;
        }
        // writing files bumps nothing on the directory; restore its times
        self.set_entry_info(&save.name, None, Some(save.created), Some(save.modified))?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Consistency check
    // ------------------------------------------------------------------

    /// Walk the filesystem and report problems (cross-linked or lost
    /// clusters, broken chains). Returns human readable findings; an
    /// empty list means the card looks healthy.
    pub fn check(&self) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        let n = self.sb.alloc_end as usize;
        let mut owner: Vec<Option<String>> = vec![None; n];
        let mut stack = vec![("/".to_string(), self.root_loc())];
        while let Some((path, loc)) = stack.pop() {
            let ent = match self.entry_at(loc) {
                Ok(e) => e,
                Err(e) => {
                    problems.push(format!("{path}: {e}"));
                    continue;
                }
            };
            let clusters = if ent.mode.is_dir() {
                ent.length.div_ceil(self.ents_per_cluster()) as usize
            } else {
                (ent.length as usize).div_ceil(self.cluster_size)
            };
            match self.chain_n(ent.cluster, clusters) {
                Ok(chain) => {
                    for c in chain {
                        let slot = &mut owner[c as usize];
                        if let Some(prev) = slot {
                            problems.push(format!("cluster {c} cross-linked: {prev} and {path}"));
                        } else {
                            *slot = Some(path.clone());
                        }
                    }
                }
                Err(e) => problems.push(format!("{path}: {e}")),
            }
            if ent.mode.is_dir() {
                if let Ok(children) = self.dir_entries(loc) {
                    for (i, c) in children.into_iter().skip(2) {
                        if c.mode.exists() {
                            let p = if path == "/" { format!("/{}", c.name) } else { format!("{path}/{}", c.name) };
                            stack.push((p, Loc { dir: ent.cluster, index: i }));
                        }
                    }
                }
            }
        }
        let mut lost = 0;
        for c in 0..n as u32 {
            if self.get_fat(c)? & FAT_ALLOCATED != 0 && owner[c as usize].is_none() {
                lost += 1;
            }
        }
        if lost > 0 {
            problems.push(format!("{lost} lost cluster(s) allocated but not used by any file"));
        }
        // ECC
        if self.ecc {
            let pages = self.sb.clusters_per_card as usize * self.ppc;
            let mut bad = 0;
            let spare_len = self.page_size / 32;
            for p in 0..pages {
                let o = p * self.raw_page;
                let page = &self.data[o..o + self.page_size];
                let spare = &self.data[o + self.page_size..o + self.page_size + spare_len];
                if spare.iter().all(|&b| b == 0xFF) {
                    continue; // erased page
                }
                let mut sp = vec![0u8; spare_len];
                ecc::ecc_page(page, &mut sp);
                if sp[..12] != spare[..12] {
                    bad += 1;
                }
            }
            if bad > 0 {
                problems.push(format!("{bad} page(s) with ECC mismatch"));
            }
        }
        Ok(problems)
    }

    /// Release lost clusters found by `check`.
    pub fn reclaim_lost_clusters(&mut self) -> Result<usize> {
        let n = self.sb.alloc_end as usize;
        let mut used = vec![false; n];
        let mut stack = vec![self.root_loc()];
        while let Some(loc) = stack.pop() {
            let ent = self.entry_at(loc)?;
            let clusters = if ent.mode.is_dir() {
                ent.length.div_ceil(self.ents_per_cluster()) as usize
            } else {
                (ent.length as usize).div_ceil(self.cluster_size)
            };
            for c in self.chain_n(ent.cluster, clusters)? {
                used[c as usize] = true;
            }
            if ent.mode.is_dir() {
                for (i, c) in self.dir_entries(loc)?.into_iter().skip(2) {
                    if c.mode.exists() {
                        stack.push(Loc { dir: ent.cluster, index: i });
                    }
                }
            }
        }
        let mut freed = 0;
        for c in 0..n as u32 {
            let v = self.get_fat(c)?;
            if v & FAT_ALLOCATED != 0 && !used[c as usize] {
                self.set_fat(c, v & FAT_MASK)?;
                freed += 1;
            }
        }
        Ok(freed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_layout() {
        let c = MemoryCard::format(8, true).unwrap();
        assert_eq!(c.as_bytes().len(), STANDARD_SIZE_ECC);
        assert_eq!(c.sb.alloc_offset, 41);
        assert_eq!(c.sb.alloc_end, 8135);
        assert_eq!(c.sb.backup_block1, 1023);
        assert_eq!(c.sb.backup_block2, 1022);
        let c2 = MemoryCard::open(c.as_bytes().to_vec()).unwrap();
        assert!(c2.has_ecc());
        assert!(c2.list("/").unwrap().is_empty());
        let raw = MemoryCard::format(8, false).unwrap();
        assert_eq!(raw.as_bytes().len(), STANDARD_SIZE_RAW);
    }

    #[test]
    fn files_and_dirs() {
        let mut c = MemoryCard::format(8, true).unwrap();
        let free0 = c.free_clusters().unwrap();
        c.mkdir("BESLES-12345").unwrap();
        let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        c.write_file("BESLES-12345/data.bin", &data).unwrap();
        c.write_file("BESLES-12345/empty", &[]).unwrap();
        for i in 0..7 {
            c.write_file(&format!("BESLES-12345/f{i}"), &[i as u8; 10]).unwrap();
        }
        assert_eq!(c.read_file("/BESLES-12345/data.bin").unwrap(), data);
        assert_eq!(c.read_file("BESLES-12345/empty").unwrap(), Vec::<u8>::new());
        assert_eq!(c.list("BESLES-12345").unwrap().len(), 9);
        // overwrite smaller
        c.write_file("BESLES-12345/data.bin", b"hello").unwrap();
        assert_eq!(c.read_file("BESLES-12345/data.bin").unwrap(), b"hello");
        let reopened = MemoryCard::open(c.as_bytes().to_vec()).unwrap();
        assert_eq!(reopened.read_file("BESLES-12345/f6").unwrap(), vec![6u8; 10]);
        assert!(c.check().unwrap().is_empty(), "{:?}", c.check());
        assert!(matches!(c.remove("BESLES-12345"), Err(Error::NotEmpty(_))));
        c.remove_all("BESLES-12345").unwrap();
        // the root directory keeps the cluster it grew into
        assert_eq!(c.free_clusters().unwrap(), free0 - 1);
        assert!(c.check().unwrap().is_empty());
        // deleted slot reuse
        c.mkdir("A").unwrap();
        assert_eq!(c.list("/").unwrap().len(), 1);
    }

    #[test]
    fn fill_card() {
        let mut c = MemoryCard::format(8, false).unwrap();
        c.mkdir("BIG").unwrap();
        let chunk = vec![0xAB; 1024 * 1024];
        let mut i = 0;
        loop {
            match c.write_file(&format!("BIG/{i}"), &chunk) {
                Ok(()) => i += 1,
                Err(Error::NoSpace { .. }) => break,
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(i, 7);
        assert!(c.check().unwrap().is_empty(), "{:?}", c.check());
    }

    #[test]
    fn ecc_conversion() {
        let mut c = MemoryCard::format(8, false).unwrap();
        c.mkdir("X").unwrap();
        c.write_file("X/y", b"abc").unwrap();
        let e = c.to_bytes_with_ecc(true);
        let c2 = MemoryCard::open(e).unwrap();
        assert!(c2.has_ecc());
        assert_eq!(c2.read_file("X/y").unwrap(), b"abc");
        assert!(c2.check().unwrap().is_empty());
    }
}
