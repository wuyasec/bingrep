extern crate goblin;
extern crate colored;
extern crate structopt;
#[macro_use]
extern crate structopt_derive;
extern crate rustc_demangle;
extern crate scroll;
#[macro_use]
extern crate prettytable;
extern crate term;

use scroll::*;
use prettytable::{format, Table};
use prettytable::row::Row;
use prettytable::cell::Cell;

use goblin::{error, Hint, pe, elf, mach, archive, container};
use std::path::Path;
use std::fs::File;
use std::io::Read;

use colored::Colorize;
use structopt::StructOpt;

#[derive(StructOpt, Debug, Clone)]
#[structopt(name = "bingrep", about = "bingrep - grepping through binaries since 2017")]
struct Opt {

    /// A flag, true if used in the command line.
    #[structopt(short = "d", long = "debug", help = "Print debug version of parse results")]
    debug: bool,

    /// Whether to use pretty tables
    #[structopt(short = "p", long = "pretty", help = "Use pretty tables")]
    pretty: bool,

    /// Force coloring
    #[structopt(long = "color", help = "Color")]
    color: bool,

    ///
    #[structopt(short = "s", long = "search", help = "Search for string")]
    search: Option<String>,

    /// A flag, true if used in the command line.
    #[structopt(short = "D", long = "demangle", help = "Apply Rust/C++ demangling")]
    demangle: bool,

    /// Needed parameter, the first on the command line.
    #[structopt(help = "Binary file")]
    input: String,
}

fn new_table(title: Row) -> Table {
    let sep = format::LineSeparator::new('-', '|', ' ', ' ');

    let format = format::FormatBuilder::new()
        .column_separator(' ')
        .borders(' ')
        .separators(&[], sep)
        .padding(1, 1)
        .build();

    let mut phdr_table = Table::new();
    phdr_table.set_titles(title);
    phdr_table.set_format(format);
    phdr_table
}

fn string_cell (opt: &Opt, s: &str) -> Cell {
    if s.is_empty() {
        Cell::new(&"")
    } else {
        Cell::new(&if opt.demangle {
            rustc_demangle::demangle(s).to_string()
        } else {
            s.into()
        }).style_spec("FYb")
    }
}

fn idx_cell (i: usize) -> Cell {
    let cell = Cell::new(&i.to_string());
    if i % 2 == 0 { cell.style_spec("FdBw") } else { cell.style_spec("FwBd") }
}

fn addr_cell (addr: u64) -> Cell {
    Cell::new(&format!("{:>16x} ", addr)).style_spec("Frr")
}

fn offsetx_cell (offset: u64) -> Cell {
    Cell::new(&format!("{:#x} ", offset)).style_spec("Fy")
}

fn addrx_cell (addr: u64) -> Cell {
    Cell::new(&format!("{:#x} ", addr)).style_spec("Fr")
}

fn memx_cell (maddr: u64) -> Cell {
    Cell::new(&format!("{:<#x} ", maddr)).style_spec("bFr")
}

fn sz_cell (size: u64) -> Cell {
    Cell::new(&format!("{:<#x} ", size)).style_spec("Fg")
}

fn memsz_cell (memsz: u64) -> Cell {
    Cell::new(&format!("{:<#x} ", memsz)).style_spec("bFg")
}

fn x_cell (num: u64) -> Cell {
    Cell::new(&format!("{:#x}", num))
}

fn shndx_cell (idx: usize, shdrs: &elf::SectionHeaders, strtab: &goblin::strtab::Strtab) -> Cell {
    if idx >= shdrs.len() {
        if idx == 0xfff1 { // associated symbol is absolute, todo, move this to goblin
            Cell::new(&format!("ABS")).style_spec("iFw")
        } else {
            Cell::new(&format!("BAD_IDX={}", idx)).style_spec("irFw")
        }
    } else if idx != 0 {
        let shdr = &shdrs[idx];
        let link_name = &strtab[shdr.sh_name];
        Cell::new(&format!("{}({})", link_name, idx))
    } else {
        Cell::new("")
    }
}

fn hdr(name: &str) -> colored::ColoredString {
    format!("{}", name).dimmed().white().underline()
}

fn hdr_size (name: &str, size: usize) -> colored::ColoredString {
    format!("{}({})", name, size).dimmed().white().underline()
}

fn fmt_header (fmt: &mut ::std::fmt::Formatter, name: &str, size: usize) -> ::std::fmt::Result {
    writeln!(fmt, "{}:\n", hdr_size(name, size))?;
    Ok(())
}

fn addr (addr: u64) -> colored::ColoredString {
    format!("{:x}",addr).red()
}

fn addrx (addr: u64) -> colored::ColoredString {
    format!("{:#x}",addr).red()
}

fn off (off: u64) -> colored::ColoredString {
    format!("{:#x}",off).yellow()
}

fn offs (off: isize) -> colored::ColoredString {
    format!("{:#x}",off).yellow()
}

fn string (opt: &Opt, s: &str) -> colored::ColoredString {
    if opt.demangle {
        rustc_demangle::demangle(s).to_string()
    } else {
        s.into()
    }.reverse().bold().yellow()
}

fn sz (sz: u64) -> colored::ColoredString {
    format!("{:#x}", sz).green()
}

fn idx (i: usize) -> colored::ColoredString {
    let index = format!("{:>4}", i);
    if i % 2 == 0 { index.white().on_black() } else { index.black().on_white() }
}

struct MachO<'a>(mach::MachO<'a>, Opt);

impl<'a> ::std::fmt::Display for MachO<'a> {
    fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        use mach::header;
        use mach::load_command;
        use mach::exports::{Export};

        let mach = &self.0;
        let opt = &self.1;
        let header = &mach.header;
        let endianness = if header.is_little_endian() { "little-endian" } else { "big-endian" };
        let kind = {
            let typ_cell = header.filetype;
            let kind_str = header::filetype_to_str(typ_cell).reverse().bold();
            match typ_cell {
                header::MH_OBJECT =>  kind_str.yellow(),
                header::MH_EXECUTE => kind_str.red(),
                header::MH_DYLIB =>  kind_str.blue(),
                header::MH_DYLINKER =>  kind_str.yellow(),
                header::MH_DYLIB_STUB =>  kind_str.blue(),
                header::MH_DSYM =>  kind_str.green(),
                header::MH_CORE => kind_str.black(),
                _ => kind_str.normal(),
            }
        };
        let machine = header.cputype;
        let machine_str = {
            mach::constants::cputype::cpu_type_to_str(machine).bold()
        };
        writeln!(fmt, "{} {} {}-{} @ {}:",
                 hdr("Mach-o"),
                 kind,
                 machine_str,
                 endianness,
                 addrx(mach.entry as u64),
        )?;
        writeln!(fmt, "")?;

        let lcs = &mach.load_commands;
        fmt_header(fmt, "LoadCommands", mach.load_commands.len())?;
        for (i, lc) in lcs.into_iter().enumerate() {
            let name = {
                let name = load_command::cmd_to_str(lc.command.cmd());
                let name = format!("{:.27}", name);
                match lc.command {
                    load_command::CommandVariant::Segment32        (_command) => name.red(),
                    load_command::CommandVariant::Segment64        (_command) => name.red(),
                    load_command::CommandVariant::Symtab           (_command) => name.yellow(),
                    load_command::CommandVariant::Dysymtab         (_command) => name.green(),
                    load_command::CommandVariant::LoadDylinker     (_command) => name.yellow(),
                    load_command::CommandVariant::LoadDylib        (_command)
                    | load_command::CommandVariant::LoadUpwardDylib(_command)
                    | load_command::CommandVariant::ReexportDylib  (_command)
                    | load_command::CommandVariant::LazyLoadDylib  (_command) => name.blue(),
                    load_command::CommandVariant::DyldInfo         (_command)
                    | load_command::CommandVariant::DyldInfoOnly   (_command) => name.cyan(),
                    load_command::CommandVariant::Unixthread       (_command) => name.red(),
                    load_command::CommandVariant::Main             (_command) => name.red(),
                    _ => name.normal(),
                }
            };
            write!(fmt, "{} ", idx(i))?;
            writeln!(fmt, "{:<27} ", name)?;
        }

        writeln!(fmt, "")?;
        let fmt_section = |fmt: &mut ::std::fmt::Formatter, i: usize, section: &load_command::Section | -> ::std::fmt::Result {
            if let Ok(name) = section.name() {
                write!(fmt,   "    {}: {:>16}", idx(i), string(opt, name))?;
                write!(fmt,   "    addr: {:>8} ",     addr(section.addr))?;
                write!(fmt,   "    size: {:>8} ",     sz(section.size))?;
                write!(fmt,   "    offset: {:>8} ",   off(section.offset as u64))?;
                write!(fmt,   "    align: {} ",    section.align)?;
                write!(fmt,   "    reloff: {} ",   off(section.reloff as u64))?;
                write!(fmt,   "    nreloc: {} ",   section.nreloc)?;
                write!(fmt,   "    flags: {:#10x} ",    section.flags)?;
                writeln!(fmt, "    data: {}",    section.data.len())
            } else {
                writeln!(fmt,   "    {}: {:>16}", idx(i), "BAD SECTION NAME")
            }
        };

        let fmt_sections = |fmt: &mut ::std::fmt::Formatter, name: &str, sections: &[load_command::Section] | -> ::std::fmt::Result {
            writeln!(fmt, "  {}", hdr_size(name, sections.len()).yellow())?;
            for (i, section) in sections.into_iter().enumerate() {
                fmt_section(fmt, i, &section)?;
            }
            Ok(())
        };

        let segments = &*mach.segments;
        fmt_header(fmt, "Segments", segments.len())?;
        for (ref i, ref segment) in segments.into_iter().enumerate() {
            write!(fmt, "  {}:",     (*i).to_string().yellow())?;
            let name = segment.name().unwrap();
            fmt_sections(fmt, name, &segment.sections().unwrap())?;
        }

        writeln!(fmt, "")?;

        let fmt_exports = |fmt: &mut ::std::fmt::Formatter, name: &str, syms: &[Export] | -> ::std::fmt::Result {
            fmt_header(fmt, name, syms.len())?;
            for sym in syms {
                write!(fmt, "{:>16} ", addr(sym.offset))?;
                write!(fmt, "{} ", string(opt, &sym.name))?;
                writeln!(fmt, "({})", sz(sym.size as u64))?;
            }
            writeln!(fmt, "")
        };

        let exports = match mach.exports () { Ok(exports) => exports, Err(_) => Vec::new() };
        fmt_exports(fmt, "Exports", &exports)?;

        let imports = match mach.imports () { Ok(imports) => imports, Err(_) => Vec::new() };
        fmt_header(fmt, "Imports", imports.len())?;
        for sym in imports {
            write!(fmt, "{:>16} ", addr(sym.offset))?;
            write!(fmt, "{} ", string(opt, &sym.name))?;
            write!(fmt, "({})", sz(sym.size as u64))?;
            writeln!(fmt, "-> {}", string(opt, sym.dylib).blue())?;
        }
        writeln!(fmt, "")?;

        fmt_header(fmt, "Libraries", mach.libs.len())?;
        for lib in &mach.libs[1..] {
            writeln!(fmt, "{:>16} ", string(opt, lib).blue())?;
        }
        writeln!(fmt, "")?;

        writeln!(fmt, "Name: {}", if let &Some(ref name) = &mach.name{ name } else { "None" })?;
        writeln!(fmt, "is_64: {}", mach.header.container() == container::Container::Big )?;
        writeln!(fmt, "is_lib: {}", mach.header.filetype == header::MH_DYLIB)?;
        writeln!(fmt, "little_endian: {}", mach.header.is_little_endian())?;
        writeln!(fmt, "entry: {}", addr(mach.entry as u64))?;

        Ok(())
    }
}

struct Elf<'a> {
    elf: elf::Elf<'a>,
    bytes: &'a [u8],
    opt: Opt,
}

impl<'a> ::std::fmt::Display for Elf<'a> {
    fn fmt(&self, fmt: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        use elf::header;
        use elf::program_header;
        use elf::section_header;
        use elf::sym;
        use elf::dyn;
        use elf::Dynamic;
        use elf::Syms;
        use elf::strtab::Strtab;
        use elf::reloc::{self, Reloc};
        use colored::*;
        let header = &self.elf.header;
        let endianness = if self.elf.little_endian { "little-endian" } else { "big-endian" };
        let kind = {
            let typ_cell = header.e_type;
            let kind_str = header::et_to_str(typ_cell).reverse().bold();
            match typ_cell {
                header::ET_REL =>  kind_str.yellow(),
                header::ET_EXEC => kind_str.red(),
                header::ET_DYN =>  kind_str.blue(),
                header::ET_CORE => kind_str.black(),
                _ => kind_str.normal(),
            }
        };
        let machine = header.e_machine;
        let machine_str = {
            header::machine_to_str(machine).bold()
        };
        writeln!(fmt, "{} {} {}-{} @ {}:",
                 hdr("ELF"),
                 kind,
                 machine_str,
                 endianness,
                 addrx(self.elf.entry as u64),
        )?;
        writeln!(fmt, "")?;
        writeln!(fmt, "e_phoff: {} e_shoff: {} e_flags: {:#x} e_ehsize: {} e_phentsize: {} e_phnum: {} e_shentsize: {} e_shnum: {} e_shstrndx: {}",
                 off(header.e_phoff),
                 off(header.e_shoff),
                 header.e_flags,
                 header.e_ehsize,
                 header.e_phentsize,
                 header.e_phnum,
                 header.e_shentsize,
                 header.e_shnum,
                 header.e_shstrndx,
        )?;
        writeln!(fmt, "")?;

        let ph_name = |phdr: &elf::ProgramHeader| {
            let typ_cell = phdr.p_type;
            let name = format!("{:.16}", program_header::pt_to_str(typ_cell));
            match typ_cell {
                program_header::PT_LOAD    => name.red(),
                program_header::PT_INTERP  => name.yellow(),
                program_header::PT_DYNAMIC => name.cyan(),
                _ => name.normal()
            }
        };

        let ph_flag = |phdr: &elf::ProgramHeader| {
            let wx = program_header::PF_W|program_header::PF_X;
            let rx = program_header::PF_R|program_header::PF_X;
            let rwx = program_header::PF_R|program_header::PF_W|program_header::PF_X;
            let rw = program_header::PF_R|program_header::PF_W;
            let flags = phdr.p_flags;
            if flags == rwx { "RW+X".to_owned() }
            else if flags == rw { "RW".to_owned() }
            else if flags == rx { "R+X".to_owned() }
            else if flags == wx { "W+X".to_owned() }
            else if flags == program_header::PF_R { "R".to_owned() }
            else if flags == program_header::PF_W { "W".to_owned() }
            else if flags == program_header::PF_R { "R".to_owned() }
            else { format!("{:#x}", flags) }
        };

        fmt_header(fmt, "ProgramHeaders", self.elf.program_headers.len())?;
        let phdrs = &self.elf.program_headers;
        if self.opt.pretty {
            let mut phdr_table = new_table(row![b->"Idx", b->"Type", b->"Flags", b->"Offset", b->"Vaddr", b->"Paddr", b->"Filesz", b->"Memsz", b->"Align"]);
            let ph_name_table = |phdr: &elf::ProgramHeader| {
                let typ_cell = phdr.p_type;
                let name = program_header::pt_to_str(typ_cell);
                match typ_cell {
                    program_header::PT_LOAD    => Cell::new(name).style_spec("Fr"),
                    program_header::PT_INTERP  => Cell::new(name).style_spec("Fy"),
                    program_header::PT_DYNAMIC => Cell::new(name).style_spec("Fc"),
                    _ =>  Cell::new(name),
                }
            };
            for (i, phdr) in phdrs.into_iter().enumerate() {
                let name_cell = ph_name_table(&phdr);
                let flags = ph_flag(&phdr);
                phdr_table.add_row(Row::new(vec![
                    Cell::new(&i.to_string()),
                    name_cell,
                    Cell::new(&flags),
                    offsetx_cell(phdr.p_offset),
                    addrx_cell(phdr.p_vaddr),
                    memx_cell(phdr.p_paddr),
                    sz_cell(phdr.p_filesz),
                    memsz_cell(phdr.p_filesz),
                    x_cell(phdr.p_align),
                ]));
            }
            phdr_table.print_tty(self.opt.color);
        } else {
            for (i, phdr) in phdrs.into_iter().enumerate() {
                let name = ph_name(&phdr);
                let flags = ph_flag(&phdr);
                write!(fmt, "{} ", idx(i))?;
                write!(fmt, "{:<16} ", name)?;
                write!(fmt, "{:>4} ", flags)?;
                write!(fmt, "p_offset: {:<16} ", off(phdr.p_offset))?;
                write!(fmt, "p_vaddr: {:<16} ", addrx(phdr.p_vaddr))?;
                write!(fmt, "p_paddr: {:<16} ", addrx(phdr.p_paddr).bold())?;
                write!(fmt, "p_filesz: {:<16} ", sz(phdr.p_filesz))?;
                write!(fmt, "p_memsz: {:<16} ", sz(phdr.p_memsz).bold())?;
                write!(fmt, "p_flags: {:#x} ", phdr.p_flags)?;
                writeln!(fmt, "p_align: {:#x}", phdr.p_align)?;
            }
        }
        writeln!(fmt, "")?;

        fmt_header(fmt, "SectionHeaders", self.elf.section_headers.len())?;
        let shdr_strtab = &self.elf.shdr_strtab;
        let mut shdr_table = new_table(row![b->"Idx", b->"Name", br->"Type", b->"Flags", b->"Offset", b->"Addr", b->"Size", b->"Link", b->"Entsize", b->"Align"]);
        if self.opt.pretty {
            for (i, shdr) in (&self.elf.section_headers).into_iter().enumerate() {
                let name_cell = {
                    let name = &shdr_strtab[shdr.sh_name];
                    if i % 2 == 0 { Cell::new(name).style_spec("FdBw") } else { Cell::new(name).style_spec("FwBd") }
                };
                let flags_cell = {
                    let shflags = shdr.sh_flags as u32;
                    if shflags != 0 {
                        let mut flags = String::new();
                        for flag in &section_header::SHF_FLAGS {
                            let flag = *flag;
                            if shflags & flag == flag {
                                flags += &section_header::shf_to_str(flag).to_string().split_off(4);
                                flags += " ";
                            }
                        }
                        Cell::new(&flags).style_spec("lbW")
                    } else {
                        Cell::new("")
                    }
                };
                shdr_table.add_row(Row::new(vec![
                    idx_cell(i),
                    name_cell,
                    Cell::new(section_header::sht_to_str(shdr.sh_type)).style_spec("r"),
                    flags_cell,
                    offsetx_cell(shdr.sh_offset),
                    memx_cell(shdr.sh_addr),
                    memsz_cell(shdr.sh_size),
                    shndx_cell(shdr.sh_link as usize, &self.elf.section_headers, &self.elf.shdr_strtab),
                    x_cell(shdr.sh_entsize),
                    x_cell(shdr.sh_addralign),
                ]));
            }
            shdr_table.print_tty(self.opt.color);
        } else {
            for (i, shdr) in (&self.elf.section_headers).into_iter().enumerate() {
                let name = {
                    let name = format!("{:.16}", &shdr_strtab[shdr.sh_name]);
                    if i % 2 == 0 { name.white().on_black() } else { name.black().on_white() }
                };
                write!(fmt, "{} {:<16} ", idx(i), name)?;
                write!(fmt, "{} ", section_header::sht_to_str(shdr.sh_type))?;
                write!(fmt, "sh_offset: {} ", off(shdr.sh_offset))?;
                write!(fmt, "sh_addr: {} ", addrx(shdr.sh_addr))?;
                write!(fmt, "sh_size: {} ", sz(shdr.sh_size))?;
                write!(fmt, "sh_link: {} "   , shdr.sh_link)?;
                write!(fmt, "sh_info: {:#x} ", shdr.sh_info)?;
                write!(fmt, "sh_entsize: {:#x} ", shdr.sh_entsize)?;
                write!(fmt, "sh_flags: {:#x} ", shdr.sh_flags)?;
                write!(fmt, "sh_addralign: {:#x} ", shdr.sh_addralign)?;
                let shflags = shdr.sh_flags as u32;
                if shflags != 0 {
                    writeln!(fmt)?;
                    write!(fmt, "{:<16}", "")?;
                    for flag in &section_header::SHF_FLAGS {
                        let flag = *flag;
                        if shflags & flag == flag {
                            write!(fmt, "{} ", section_header::shf_to_str(flag).to_string().split_off(4).bold())?;
                        }
                    }
                }
                writeln!(fmt)?;
            }
        }
        writeln!(fmt, "")?;

        let fmt_syms = |fmt: &mut ::std::fmt::Formatter, name: &str, syms: &Syms, strtab: &Strtab | -> ::std::fmt::Result {
            fmt_header(fmt, name, syms.len())?;
            if self.opt.pretty {
                let mut table = new_table(row![br->"Addr", bl->"Bind", bl->"Type", b->"Symbol", b->"Size", b->"Section", b->"Other"]);
                for sym in syms {
                    let bind_cell = {
                        let bind_cell = Cell::new(&format!("{:<8}",sym::bind_to_str(sym.st_bind())));
                        match sym.st_bind() {
                            sym::STB_LOCAL => bind_cell.style_spec("bBCFD"),
                            sym::STB_GLOBAL => bind_cell.style_spec("bBRFD"),
                            sym::STB_WEAK => bind_cell.style_spec("bBMFD"),
                            _ => bind_cell
                        }
                    };
                    let typ_cell = {
                        let typ_cell = Cell::new(&format!("{:<9}", sym::type_to_str(sym.st_type())));
                        match sym.st_type() {
                            sym::STT_OBJECT => typ_cell.style_spec("bFY"),
                            sym::STT_FUNC => typ_cell.style_spec("bFR"),
                            sym::STT_GNU_IFUNC => typ_cell.style_spec("bFC"),
                            _ => typ_cell
                        }
                    };
                    table.add_row(Row::new(vec![
                        addr_cell(sym.st_value),
                        bind_cell,
                        typ_cell,
                        string_cell(&self.opt, &strtab[sym.st_name]),
                        sz_cell(sym.st_size),
                        shndx_cell(sym.st_shndx, &self.elf.section_headers, &self.elf.shdr_strtab),
                        Cell::new(&format!("{:#x} ", sym.st_other)),
                    ]));
                }
                table.print_tty(self.opt.color);
            } else {
                for sym in syms {
                    let bind = {
                        let bind_str = format!("{:.8}", sym::bind_to_str(sym.st_bind())).reverse().bold();
                        match sym.st_bind() {
                            sym::STB_LOCAL => bind_str.cyan(),
                            sym::STB_GLOBAL => bind_str.red(),
                            sym::STB_WEAK => bind_str.magenta(),
                            _ => bind_str.normal().clear(),
                        }
                    };
                    let typ_cell = {
                        let typ_str = format!("{:.9}", sym::type_to_str(sym.st_type())).bold();
                        match sym.st_type() {
                            sym::STT_OBJECT => typ_str.yellow(),
                            sym::STT_FUNC => typ_str.red(),
                            sym::STT_GNU_IFUNC => typ_str.cyan(),
                            _ => typ_str.clear(),
                        }
                    };
                    write!(fmt, "{:>16} ", addr(sym.st_value))?;
                    write!(fmt, "{:<8} {:<9} ", bind, typ_cell)?;
                    write!(fmt, "{} ", string(&self.opt, &strtab[sym.st_name]))?;
                    write!(fmt, "st_size: {} ",  sz(sym.st_size))?;
                    write!(fmt, "st_other: {:#x} ", sym.st_other)?;
                    writeln!(fmt, "st_shndx: {:#x}",sym.st_shndx)?;
                }
            }
            writeln!(fmt, "")?;
            Ok(())
        };

        let dyn_strtab = &self.elf.dynstrtab;
        let strtab = &self.elf.strtab;
        fmt_syms(fmt, "Syms", &self.elf.syms, strtab)?;
        fmt_syms(fmt, "Dyn Syms", &self.elf.dynsyms, dyn_strtab)?;

        let fmt_relocs = |fmt: &mut ::std::fmt::Formatter, relocs: &[Reloc], syms: &Syms, strtab: &Strtab | -> ::std::fmt::Result {
            for reloc in relocs {
                let sym = &syms[reloc.r_sym];
                write!(fmt, "{:>16} ", addr(reloc.r_offset as u64))?;
                let name = if sym.st_name == 0 {
                    if sym.st_type() == sym::STT_SECTION {
                        let shdr = &self.elf.section_headers[sym.st_shndx];
                        shdr_strtab[shdr.sh_name].dimmed()
                    } else {
                        "ABS".dimmed()
                    }
                } else {
                    string(&self.opt, &strtab[sym.st_name])
                };
                write!(fmt, "{} ",  reloc::r_to_str(reloc.r_type, machine))?;
                let addend = if reloc.r_addend == 0 {
                    "".normal()
                } else {
                    format!("+{}", offs(reloc.r_addend)).normal()
                };
                writeln!(fmt, "{}{}", name, addend)?;
            }
            writeln!(fmt, "")?;
            Ok(())
        };

        fmt_header(fmt, "Dynamic Relas", self.elf.dynrelas.len())?;
        fmt_relocs(fmt,  &self.elf.dynrelas, &self.elf.dynsyms, &dyn_strtab)?;
        fmt_header(fmt, "Dynamic Rel", self.elf.dynrels.len())?;
        fmt_relocs(fmt,  &self.elf.dynrels, &self.elf.dynsyms, &dyn_strtab)?;
        fmt_header(fmt, "Plt Relocations", self.elf.pltrelocs.len())?;
        fmt_relocs(fmt, &self.elf.pltrelocs, &self.elf.dynsyms, &dyn_strtab)?;

        // ewwwwww, this ain't no ocaml fold
        let num_shdr_relocs = self.elf.shdr_relocs.iter().fold(0, &|acc, &(_, ref v): &(usize, Vec<_>)| acc + v.len());
        fmt_header(fmt, "Shdr Relocations", num_shdr_relocs)?;
        if num_shdr_relocs != 0 {
            for &(idx, ref relocs) in &self.elf.shdr_relocs {
                let ref shdr = self.elf.section_headers[idx];
                let shdr = &self.elf.section_headers[shdr.sh_info as usize];
                let name = &shdr_strtab[shdr.sh_name];
                writeln!(fmt, "  {}({})", name.bold(), relocs.len())?;
                fmt_relocs(fmt, &relocs.as_slice(), &self.elf.syms, &strtab)?;
            }
        }

        if let &Some(Dynamic { ref dyns, .. }) = &self.elf.dynamic {
            fmt_header(fmt, "Dynamic", dyns.len())?;
            for dyn in dyns {
                let tag = dyn.d_tag;
                let val = dyn.d_val;
                let tag_str = dyn::tag_to_str(tag).cyan();
                write!(fmt, "{:>16} ", tag_str)?;
                match tag {
                    dyn::DT_RPATH => writeln!(fmt, "{}", string(&self.opt, &dyn_strtab[val as usize]))?,
                    dyn::DT_NEEDED => writeln!(fmt, "{}", string(&self.opt, &dyn_strtab[val as usize]))?,
                    dyn::DT_INIT => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_FINI => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_INIT_ARRAY => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_INIT_ARRAYSZ => writeln!(fmt, "{}", sz(val))?,
                    dyn::DT_FINI_ARRAY => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_FINI_ARRAYSZ => writeln!(fmt, "{}", sz(val))?,
                    dyn::DT_GNU_HASH => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_STRTAB => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_SYMTAB => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_STRSZ => writeln!(fmt, "{}", sz(val))?,
                    dyn::DT_PLTGOT => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_PLTRELSZ => writeln!(fmt, "{}", sz(val))?,
                    dyn::DT_JMPREL => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_RELA => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_RELASZ => writeln!(fmt, "{}", sz(val))?,
                    dyn::DT_VERNEED => writeln!(fmt, "{}", addrx(val))?,
                    dyn::DT_VERSYM => writeln!(fmt, "{}", addrx(val))?,
                    _ => writeln!(fmt, "{:#x}", dyn.d_val)?,
                }
            }
        } else {
            writeln!(fmt, "{}: None", hdr("Dynamic"))?;
        }
        writeln!(fmt, "")?;

        fmt_header(fmt, "Libraries", self.elf.libraries.len())?;
        for lib in &self.elf.libraries {
            writeln!(fmt, "{:>16} ", string(&self.opt, lib).blue())?;
        }
        writeln!(fmt, "")?;

        writeln!(fmt, "Soname: {:?}", self.elf.soname)?;
        writeln!(fmt, "Interpreter: {}", if let &Some(ref interpreter) = &self.elf.interpreter{ interpreter } else { "None" })?;
        writeln!(fmt, "is_64: {}", self.elf.is_64)?;
        writeln!(fmt, "is_lib: {}", self.elf.is_lib)?;
        writeln!(fmt, "little_endian: {}", self.elf.little_endian)?;
        writeln!(fmt, "bias: {:#x}", self.elf.bias)?;
        writeln!(fmt, "entry: {}", addr(self.elf.entry as u64))?;

        match self.opt.search {
            Some(ref search) => {
                let mut matches = Vec::new();
                for i in 0..self.bytes.len() {
                    match self.bytes.pread_slice::<str>(i, search.len()) {
                        Ok(res) => {
                            if res == search {
                                matches.push(i);
                            }
                        },
                        _ => (),
                    }
                }

                writeln!(fmt)?;
                writeln!(fmt, "Matches for {:?}:", search)?;
                let _match_table = new_table(row!["Phdr", "Shdr"]);
                let normalize = |offset: usize, base_offset: u64, base: u64| -> u64 {
                    (offset as u64 - base_offset) + base
                };
                for offset in matches {
                    writeln!(fmt, "  {:#x}", offset)?;
                    let shdr_strtab = &self.elf.shdr_strtab;
                    for (i, phdr) in phdrs.into_iter().enumerate() {
                        if offset as u64 >= phdr.p_offset && (offset as u64) < (phdr.p_offset + phdr.p_filesz) {
                            writeln!(fmt, "  ├──{}({}) ∈ {}", program_header::pt_to_str(phdr.p_type), i, format!("{:#x}", normalize(offset, phdr.p_offset, phdr.p_vaddr)).red())?;
                        }
                    }
                    for (i, shdr) in (&self.elf.section_headers).into_iter().enumerate() {
                        if offset as u64 >= shdr.sh_offset && (offset as u64) < (shdr.sh_offset + shdr.sh_size) {
                            writeln!(fmt, "  ├──{}({}) ∈ {}", &shdr_strtab[shdr.sh_name], i, format!("{:#x}", normalize(offset, shdr.sh_offset, shdr.sh_addr)).red())?;
                            // use prettytable::Slice;
                            // let slice = shdr_table.slice(i..i+1);
                            // slice.printstd();
                        }
                    }
                }
            },
            None => ()
        }

        Ok(())
    }
}

fn run (opt: Opt) -> error::Result<()> {
    let path = Path::new(&opt.input);
    let mut fd = File::open(path)?;
    let peek = goblin::peek(&mut fd)?;
    if let Hint::Unknown(magic) = peek {
        println!("unknown magic: {:#x}", magic)
    } else {
        let bytes = { let mut v = Vec::new(); fd.read_to_end(&mut v)?; v };
        match peek {
            Hint::Elf(_) => {
                let elf = elf::Elf::parse(&bytes)?;
                if opt.debug {
                    println!("{:#?}", elf);
                } else {
                    println!("{}", Elf {elf: elf, opt: opt.clone(), bytes: bytes.as_slice()});
                }
            },
            Hint::PE => {
                let pe = pe::PE::parse(&bytes)?;
                println!("pe: {:#?}", &pe);
            },
            Hint::MachFat(_) => {
                let mach = mach::Mach::parse(&bytes)?;
                if opt.debug {
                    println!("{:#?}", mach);
                } else {
                    match mach {
                        mach::Mach::Fat(multi) => {
                            for i in 0..multi.narches {
                                match multi.get(i) {
                                    Ok(binary) => {
                                        println!("{}", MachO(binary, opt.clone()));
                                    },
                                    Err(err) => {
                                        println!("{}", err);
                                    }
                                }
                            }
                        },
                        mach::Mach::Binary(binary) => {
                            println!("{}", MachO(binary, opt.clone()));
                        }
                    }
                }
            }
            Hint::Mach(_) => {
                let mach = mach::MachO::parse(&bytes, 0)?;
                if opt.debug {
                    println!("{:#?}", mach);
                } else {
                    println!("{}", MachO(mach, opt.clone()));
                }
             },
            Hint::Archive => {
                let archive = archive::Archive::parse(&bytes)?;
                println!("archive: {:#?}", &archive);
            },
            _ => unreachable!()
        }
    }
    Ok(())
}

pub fn main () {
    let opt = Opt::from_args();
    match run(opt) {
        Ok(()) => (),
        Err(err) => println!("{:#}", err)
    }
}
