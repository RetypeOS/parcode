//! # Parcode Derive Macros
//!
//! This crate provides the procedural macros for `parcode`. It automates the implementation
//! of the `ParcodeVisitor`, `SerializationJob`, and `ParcodeNative` traits for user-defined structs.
//!
//! ## Architecture
//! The macro implements the "Surgical Serialization" strategy:
//! 1. **Local Fields:** Serialized into the current chunk via `bincode`.
//! 2. **Remote Fields:** Marked with `#[parcode(chunkable)]`, these become child nodes in the graph.
//!
//! Compatible with `syn 2.0`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Data, Attribute, LitStr};

/// Derives `ParcodeVisitor`, `SerializationJob`, and `ParcodeNative`.
#[proc_macro_derive(ParcodeObject, attributes(parcode))]
pub fn derive_parcode_object(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    
    // 1. Validation: Only structs are supported.
    let data_struct = match input.data {
        Data::Struct(ds) => ds,
        _ => return syn::Error::new(name.span(), "ParcodeObject only supports structs").to_compile_error().into(),
    };

    // 2. Field Classification
    let mut locals = Vec::new();
    let mut remotes = Vec::new();

    for field in data_struct.fields {
        // Parse attributes. Propagate compilation errors if syntax is invalid.
        let (is_chunkable, compression_id) = match parse_attributes(&field.attrs) {
            Ok(res) => res,
            Err(e) => return e.to_compile_error().into(),
        };
        
        if is_chunkable {
            remotes.push(RemoteField { 
                ident: field.ident.clone().unwrap(), 
                ty: field.ty.clone(),
                compression_id 
            });
        } else {
            locals.push(LocalField { 
                ident: field.ident.clone().unwrap(),
                ty: field.ty.clone()
            });
        }
    }

    // 3. Code Generation
    let impl_visitor = generate_visitor(&name, &remotes, &locals);
    let impl_job = generate_serialization_job(&name, &locals);
    let impl_native = generate_native_reader(&name, &locals, &remotes);

    // 4. Expansion
    let expanded = quote! {
        #impl_visitor
        #impl_job
        #impl_native
    };

    TokenStream::from(expanded)
}

// --- Internal Data Structures ---

struct LocalField {
    ident: syn::Ident,
    #[allow(dead_code)] ty: syn::Type,
}

struct RemoteField {
    ident: syn::Ident,
    ty: syn::Type,
    compression_id: u8,
}

// --- Parsing Logic (Syn 2.0) ---

/// Parses `#[parcode(...)]` attributes.
/// Returns `(is_chunkable, compression_id)`.
fn parse_attributes(attrs: &[Attribute]) -> syn::Result<(bool, u8)> {
    let mut is_chunkable = false;
    let mut compression_id = 0;

    for attr in attrs {
        if attr.path().is_ident("parcode") {
            // Use parse_nested_meta to iterate over comma-separated arguments
            attr.parse_nested_meta(|meta| {
                // Case: #[parcode(chunkable)]
                if meta.path.is_ident("chunkable") {
                    is_chunkable = true;
                    return Ok(());
                }
                
                // Case: #[parcode(compression = "lz4")]
                if meta.path.is_ident("compression") {
                    let value = meta.value()?; // Expects ' = '
                    let s: LitStr = value.parse()?; // Expects string literal
                    
                    compression_id = match s.value().to_lowercase().as_str() {
                        "lz4" => 1,
                        "zstd" => 2, // Reserved
                        "none" => 0,
                        _ => return Err(meta.error("Unknown compression algorithm. Supported: 'lz4', 'none'")),
                    };
                    return Ok(());
                }

                // Error on unknown keys
                Err(meta.error("Unknown parcode attribute key"))
            })?;
        }
    }
    Ok((is_chunkable, compression_id))
}

// --- Generator: ParcodeVisitor ---

fn generate_visitor(name: &syn::Ident, remotes: &[RemoteField], _locals: &[LocalField]) -> proc_macro2::TokenStream {
    let visit_children = remotes.iter().map(|f| {
        let fname = &f.ident;
        let cid = f.compression_id;
        
        // Generate Config Struct if needed
        let config_expr = if cid > 0 {
            quote! { Some(parcode::graph::JobConfig { compression_id: #cid }) }
        } else {
            quote! { None }
        };

        quote! {
            self.#fname.visit(graph, Some(my_id), #config_expr);
        }
    });

    quote! {
        impl parcode::visitor::ParcodeVisitor for #name {
            fn visit(&self, graph: &mut parcode::graph::TaskGraph, parent_id: Option<parcode::graph::ChunkId>, config_override: Option<parcode::graph::JobConfig>) {
                // 1. Create Job for Self (applies override if passed from parent)
                let job = self.create_job(config_override);
                let my_id = graph.add_node(job);
                
                // 2. Link to Parent
                if let Some(pid) = parent_id {
                    graph.link_parent_child(pid, my_id);
                }
                
                // 3. Visit Children (Remote Fields)
                #(#visit_children)*
            }

            fn create_job(&self, config_override: Option<parcode::graph::JobConfig>) -> Box<dyn parcode::graph::SerializationJob> {
                let base_job = Box::new(self.clone());
                
                // Wrap with Runtime Config if needed
                if let Some(cfg) = config_override {
                    Box::new(parcode::rt::ConfiguredJob::new(base_job, cfg))
                } else {
                    base_job
                }
            }
        }
    }
}

// --- Generator: SerializationJob ---

fn generate_serialization_job(name: &syn::Ident, locals: &[LocalField]) -> proc_macro2::TokenStream {
    let serialize_stmts = locals.iter().map(|f| {
        let fname = &f.ident;
        // Use internal re-export to avoid dependency issues in user crate
        quote! {
            parcode::internal::bincode::serde::encode_into_std_write(
                &self.#fname, 
                &mut writer, 
                parcode::internal::bincode::config::standard()
            ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;
        }
    });

    quote! {
        impl parcode::graph::SerializationJob for #name {
            fn execute(&self, _children_refs: &[parcode::format::ChildRef]) -> parcode::Result<Vec<u8>> {
                let mut buffer = Vec::new();
                // Use BufWriter for efficiency
                let mut writer = std::io::BufWriter::new(&mut buffer);
                
                // Serialize ONLY local fields
                #(#serialize_stmts)*
                
                use std::io::Write;
                writer.flush()?;
                drop(writer);
                
                Ok(buffer)
            }

            fn estimated_size(&self) -> usize {
                std::mem::size_of::<Self>()
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
    }
}

// --- Generator: ParcodeNative (Reader) ---

fn generate_native_reader(name: &syn::Ident, locals: &[LocalField], remotes: &[RemoteField]) -> proc_macro2::TokenStream {
    // 1. Read Local Fields (Must match serialization order)
    let read_locals = locals.iter().map(|f| {
        let fname = &f.ident;
        let fty = &f.ty;
        quote! {
            let #fname: #fty = parcode::internal::bincode::serde::decode_from_std_read(
                &mut reader, 
                parcode::internal::bincode::config::standard()
            ).map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?;
        }
    });

    // 2. Read Remote Fields (Must match visit order)
    let read_remotes = remotes.iter().map(|f| {
        let fname = &f.ident;
        let fty = &f.ty;
        quote! {
            let child_node = child_iter.next().ok_or_else(|| 
                parcode::ParcodeError::Format(format!("Missing child chunk for field '{}'", stringify!(#fname)))
            )?;
            let #fname: #fty = parcode::reader::ParcodeNative::from_node(&child_node)?;
        }
    });

    // Collect all field names for struct initialization
    let mut field_names = Vec::new();
    for f in locals { field_names.push(&f.ident); }
    for f in remotes { field_names.push(&f.ident); }

    quote! {
        impl parcode::reader::ParcodeNative for #name {
            fn from_node(node: &parcode::reader::ChunkNode<'_>) -> parcode::Result<Self> {
                // A. Read Payload
                let payload = node.read_raw()?;
                let mut reader = std::io::Cursor::new(payload);

                // B. Decode Locals
                #(#read_locals)*

                // C. Get Children
                let children = node.children()?;
                let mut child_iter = children.into_iter();

                // D. Decode Remotes
                #(#read_remotes)*

                // E. Assemble
                Ok(Self {
                    #(#field_names),*
                })
            }
        }
    }
}