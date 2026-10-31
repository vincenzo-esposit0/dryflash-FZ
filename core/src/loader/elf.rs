// SPDX-License-Identifier: GPL-3.0-or-later
//! ELF symbols for crash reports and the bring-up digest (BRIEF §4.4). The ELF is never
//! executed; only its symbol table is read.

use object::{Object, ObjectSymbol, SymbolKind};

#[derive(Clone, Debug, Default)]
pub struct Symbols {
    /// (address, size, name), sorted by address. Thumb bit cleared.
    funcs: Vec<(u32, u32, String)>,
}

impl Symbols {
    pub fn from_elf(data: &[u8]) -> Result<Self, object::Error> {
        let file = object::File::parse(data)?;
        let mut funcs: Vec<(u32, u32, String)> = file
            .symbols()
            .filter(|s| matches!(s.kind(), SymbolKind::Text | SymbolKind::Data | SymbolKind::Unknown))
            .filter(|s| s.address() != 0 && s.is_definition())
            .filter_map(|s| Some(((s.address() as u32) & !1, s.size() as u32, s.name().ok()?.to_string())))
            .filter(|(_, _, n)| !n.is_empty() && !n.starts_with('$'))
            .collect();
        funcs.sort_by_key(|f| (f.0, std::cmp::Reverse(f.1)));
        funcs.dedup_by_key(|f| f.0);
        Ok(Symbols { funcs })
    }

    /// Address of a symbol by exact name.
    pub fn address_of(&self, name: &str) -> Option<u32> {
        self.funcs.iter().find(|f| f.2 == name).map(|f| f.0)
    }

    /// `name+0xoff` for an address inside a sized symbol.
    pub fn lookup(&self, addr: u32) -> Option<String> {
        let i = self.funcs.partition_point(|f| f.0 <= addr);
        let (base, size, name) = self.funcs.get(i.checked_sub(1)?)?;
        if addr - base >= (*size).max(1) {
            return None;
        }
        Some(if addr == *base {
            name.clone()
        } else {
            format!("{name}+0x{:x}", addr - base)
        })
    }

    pub fn len(&self) -> usize {
        self.funcs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.funcs.is_empty()
    }
}
