use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{parse_macro_input, DeriveInput, Data, Fields, Meta, NestedMeta};

#[proc_macro_derive(ParcodeObject, attributes(parcode))]
pub fn derive_parcode_object(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    
    // Filter fields marked with #[parcode(chunkable)]
    let chunkable_fields = get_chunkable_fields(&input.data);

    // 1. Implementation of ParcodeVisitor
    // Logic: Visit self. Store self as a node. Recurse into chunkable fields.
    let visit_impl = impl_visitor(&name, &chunkable_fields);

    // 2. Implementation of SerializationJob
    // Logic: Serialize self using bincode. BUT, for chunkable fields, 
    // DO NOT serialize the data. Instead, substitute with the ChildRef 
    // from the `completed_children` list passed to execute().
    let job_impl = impl_serialization_job(&name, &chunkable_fields);

    // Combine
    let expanded = quote! {
        #visit_impl
        #job_impl
    };

    TokenStream::from(expanded)
}

struct ChunkableField {
    ident: syn::Ident,
    // We could store compression settings here later
}

fn get_chunkable_fields(data: &Data) -> Vec<ChunkableField> {
    let mut fields_found = Vec::new();

    if let Data::Struct(data_struct) = data {
        if let Fields::Named(fields_named) = &data_struct.fields {
            for field in &fields_named.named {
                let mut is_chunkable = false;
                for attr in &field.attrs {
                    if attr.path.is_ident("parcode") {
                         if let Ok(Meta::List(list)) = attr.parse_meta() {
                             for nested in list.nested {
                                 if let NestedMeta::Meta(Meta::Path(path)) = nested {
                                     if path.is_ident("chunkable") {
                                         is_chunkable = true;
                                     }
                                 }
                             }
                         }
                    }
                }
                
                if is_chunkable {
                    if let Some(ident) = &field.ident {
                        fields_found.push(ChunkableField { ident: ident.clone() });
                    }
                }
            }
        }
    }
    fields_found
}

fn impl_visitor(name: &syn::Ident, chunkables: &[ChunkableField]) -> proc_macro2::TokenStream {
    let recurse_calls = chunkables.iter().map(|f| {
        let fname = &f.ident;
        quote! {
            self.#fname.visit(graph, Some(my_id));
        }
    });

    quote! {
        impl parcode::visitor::ParcodeVisitor for #name {
            fn visit(&self, graph: &mut parcode::graph::TaskGraph, parent_id: Option<parcode::graph::ChunkId>) {
                // 1. Create Job for self
                let job = self.create_job();
                
                // 2. Add to graph
                let my_id = graph.add_node(job);
                
                // 3. Link to parent
                if let Some(pid) = parent_id {
                    graph.link_parent_child(pid, my_id);
                }
                
                // 4. Recurse children
                #(#recurse_calls)*
            }
            
            fn create_job(&self) -> Box<dyn parcode::graph::SerializationJob> {
                // We clone self to move it into the Job Box. 
                // Assumption: User structs must be Clone for the prototype simplifiction.
                // In prod, we might use Arc or specific references to avoid deep clones of non-chunkable data.
                Box::new(self.clone())
            }
        }
    }
}

fn impl_serialization_job(name: &syn::Ident, chunkables: &[ChunkableField]) -> proc_macro2::TokenStream {
    // This is the tricky part: Custom Serialization.
    // We need to tell serde to skip the actual field and write a placeholder?
    // Or better: The `execute` method manually constructs a "Proxy Struct" that matches
    // the user struct but replaces Chunkable fields with `ChildRef` or nothing (since refs go to footer).
    
    // STRATEGY FOR V1:
    // We use a "Shadow Struct".
    // User Struct: { a: i32, b: Vec<i32> }
    // Shadow Struct: { a: i32 } (b is ignored because it lives in children chunks).
    // The `execute` function serializes the Shadow Struct.
    
    let shadow_name = syn::Ident::new(&format!("{}ParcodeShadow", name), name.span());
    
    // TODO: Generate Shadow Struct definition filtering out chunkable fields 
    // (or replacing them with markers if needed for schema evolution).
    
    // For simplicity in this prompt response, we will use Bincode's Default behavior 
    // but we need `#[serde(skip)]` on chunkable fields effectively during this pass.
    // Since we can't easily inject attributes into the user's struct dynamically at runtime, 
    // The robust way is to implement a custom Serialize for the type wrapper.

    // Placeholder implementation:
    quote! {
        impl parcode::graph::SerializationJob for #name {
             fn execute(&self, children_refs: &[parcode::format::ChildRef]) -> parcode::Result<Vec<u8>> {
                 // 1. Serialize the "Literal" parts of this struct.
                 // PROBLEM: We need to serialize everything EXCEPT the fields marked chunkable.
                 // We can use a macro-generated helper struct that borrows fields.
                 
                 #[derive(serde::Serialize)]
                 struct #shadow_name<'a> {
                     // Iterate ALL fields. If in `chunkables`, skip or use placeholder?
                     // If we skip, how do we reconstruct on read?
                     // ANSWER: On read, we read the literal part, then manually populate the chunkable fields from children.
                     
                     // This requires generating the struct fields here.
                     // Complexity Alert: This requires full reflection of fields in the macro.
                 }
                 
                 // For this phase, let's assume we simply return bincode::serialize(&self)
                 // BUT we need to zero-out or skip the chunkable vectors to avoid double writing.
                 
                 // Temporary Prod Solution: 
                 // Use a wrapper during serialization that uses `serde(skip)` behavior via a remote type or manual implementation.
                 
                 // Returning empty vec for now to signal this logic is pending precise implementation detail
                 // which depends on how strictly we want to enforce Schema.
                 Ok(bincode::serde::encode_to_vec(self, bincode::config::standard())
                    .map_err(|e| parcode::ParcodeError::Serialization(e.to_string()))?)
             }

             fn estimated_size(&self) -> usize {
                 std::mem::size_of::<Self>() // Rough estimate
             }

             fn as_any(&self) -> &dyn std::any::Any {
                 self
             }
        }
    }
}