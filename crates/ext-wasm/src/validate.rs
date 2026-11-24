//! What a module is refused for before any compile: wasmtime cannot cap or
//! cancel Cranelift, so the bound is made here, by reading the module's
//! sections once.

use wasmparser::{Parser, Payload, TypeRef};

const MAX_FUNCTIONS: u32 = 5_000;
const MAX_BODY_BYTES: u64 = 256 * 1024;
const MAX_LOCALS: u32 = 1_000;
const MAX_MEMORIES: u32 = 1;
const MAX_TABLES: u32 = 4;
const MAX_TABLE_ELEMENTS: u64 = 10_000;

/// Reads `bytes` and says why the host will not compile them, or nothing.
pub(crate) fn refuse(bytes: &[u8]) -> Option<String> {
    let mut functions: u32 = 0;
    let mut memories: u32 = 0;
    let mut tables: u32 = 0;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = match payload {
            Ok(p) => p,
            Err(e) => return Some(format!("not a WebAssembly module: {}", e.message())),
        };
        match payload {
            Payload::StartSection { .. } => return Some("a start function is refused".into()),
            Payload::ImportSection(r) => {
                for import in r.into_imports() {
                    let Ok(i) = import else {
                        return Some("unreadable import".into());
                    };
                    let known = i.module == "oss"
                        && matches!(
                            (i.name, i.ty),
                            ("log", TypeRef::Func(_)) | ("cancelled", TypeRef::Func(_))
                        );
                    if !known {
                        return Some(format!("import `{}.{}` is not offered", i.module, i.name));
                    }
                }
            }
            Payload::FunctionSection(r) => {
                functions = functions.saturating_add(r.count());
                if functions > MAX_FUNCTIONS {
                    return Some(format!("more than {MAX_FUNCTIONS} functions"));
                }
            }
            Payload::MemorySection(r) => {
                memories = memories.saturating_add(r.count());
                if memories > MAX_MEMORIES {
                    return Some("more than one memory".into());
                }
                for m in r {
                    if m.is_ok_and(|m| m.memory64 || m.shared) {
                        return Some("a 64-bit or shared memory is refused".into());
                    }
                }
            }
            Payload::TableSection(r) => {
                tables = tables.saturating_add(r.count());
                if tables > MAX_TABLES {
                    return Some(format!("more than {MAX_TABLES} tables"));
                }
                for t in r {
                    if t.is_ok_and(|t| t.ty.table64 || t.ty.initial > MAX_TABLE_ELEMENTS) {
                        return Some("a table is too large".into());
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                if body.range().end.saturating_sub(body.range().start) > MAX_BODY_BYTES {
                    return Some(format!("a function body over {MAX_BODY_BYTES} bytes"));
                }
                let Ok(locals) = body.get_locals_reader() else {
                    return Some("unreadable locals".into());
                };
                let mut total: u32 = 0;
                for l in locals {
                    let Ok((n, _)) = l else {
                        return Some("unreadable locals".into());
                    };
                    total = total.saturating_add(n);
                }
                if total > MAX_LOCALS {
                    return Some(format!("a function with more than {MAX_LOCALS} locals"));
                }
            }
            _ => {}
        }
    }
    None
}
