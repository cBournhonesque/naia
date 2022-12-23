use proc_macro2::{Punct, Spacing, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, GenericArgument, Ident, Index, Lit, Member, Meta, Path, PathArguments, Result, Type, PathSegment, parse_str};

const UNNAMED_FIELD_PREFIX: &'static str = "unnamed_field_";

pub fn replicate_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    // Helper Properties
    let properties = properties(&input);
    let is_replica_tuple_struct = is_replica_tuple_struct(&input);

    // Paths
    let (protocol_path, protocol_name) = protocol_path(&input);

    // Names
    let replica_name = input.ident;
    let protocol_kind_name = format_ident!("{}Kind", protocol_name);
    let enum_name = format_ident!("{}Property", replica_name);

    // Definitions
    let property_enum_definition = property_enum(&enum_name, &properties);

    // Replica Methods
    let new_complete_method = new_complete_method(
        &replica_name,
        &enum_name,
        &properties,
        is_replica_tuple_struct,
    );
    let read_method = read_method(
        &protocol_name,
        &replica_name,
        &enum_name,
        &properties,
        is_replica_tuple_struct,
    );
    let read_create_update_method =
        read_create_update_method(&replica_name, &protocol_kind_name, &properties);

    // ReplicateSafe Derive Methods
    let diff_mask_size = {
        let len = properties.len();
        if len == 0 {
            0
        } else {
            ((len - 1) / 8) + 1
        }
    } as u8;
    let dyn_ref_method = dyn_ref_method(&protocol_name);
    let dyn_mut_method = dyn_mut_method(&protocol_name);
    let to_protocol_method = into_protocol_method(&protocol_name, &replica_name);
    let protocol_copy_method = protocol_copy_method(&protocol_name, &replica_name);
    let clone_method = clone_method(&replica_name, &properties, is_replica_tuple_struct);
    let mirror_method = mirror_method(
        &protocol_name,
        &replica_name,
        &properties,
        is_replica_tuple_struct,
    );
    let set_mutator_method = set_mutator_method(&properties, is_replica_tuple_struct);
    let read_apply_update_method =
        read_apply_update_method(&protocol_kind_name, &properties, is_replica_tuple_struct);
    let write_method = write_method(&properties, is_replica_tuple_struct);
    let write_update_method = write_update_method(&enum_name, &properties, is_replica_tuple_struct);
    let has_entity_properties = has_entity_properties_method(&properties);
    let entities = entities_method(&properties, is_replica_tuple_struct);

    let gen = quote! {
        use std::{rc::Rc, cell::RefCell, io::Cursor};
        use naia_shared::{
            DiffMask, PropertyMutate, ReplicateSafe, PropertyMutator, ComponentUpdate,
            Protocolize, ReplicaDynRef, ReplicaDynMut, NetEntityHandleConverter,
            ReplicableProperty, ReplicableEntityProperty,
            serde::{BitReader, BitWrite, BitWriter, OwnedBitReader, Serde, SerdeErr},
        };
        use #protocol_path::{#protocol_name, #protocol_kind_name};
        mod internal {
            pub use naia_shared::{EntityProperty, EntityHandle};
        }

        #property_enum_definition

        impl #replica_name {
            #new_complete_method
            #read_method
            #read_create_update_method
        }
        impl ReplicateSafe<#protocol_name> for #replica_name {
            fn diff_mask_size(&self) -> u8 { #diff_mask_size }
            fn kind(&self) -> #protocol_kind_name {
                return Protocolize::kind_of::<Self>();
            }
            #dyn_ref_method
            #dyn_mut_method
            #to_protocol_method
            #protocol_copy_method
            #mirror_method
            #set_mutator_method
            #write_method
            #write_update_method
            #read_apply_update_method
            #has_entity_properties
            #entities
        }
        impl Replicate<#protocol_name> for #replica_name {}
        impl Clone for #replica_name {
            #clone_method
        }
    };

    proc_macro::TokenStream::from(gen)
}

pub struct NormalProperty {
    pub variable_name: Ident,
    pub inner_type: Type,
    pub uppercase_variable_name: Ident,
    /// type implementing ReplicableProperty
    pub replicable_property_type: Type,
}

pub struct EntityProperty {
    pub variable_name: Ident,
    pub uppercase_variable_name: Ident,
    /// type implementing ReplicableEntityProperty
    pub replicable_entity_property_type: Type,
}

#[allow(clippy::large_enum_variant)]
pub enum Property {
    Normal(NormalProperty),
    Entity(EntityProperty),
}

/// Create a variable name for unnamed fields
fn get_variable_name_for_unnamed_field(index: usize, span: Span) -> Ident {
    Ident::new(&format!("{}{}", UNNAMED_FIELD_PREFIX, index), span)
}

/// Get the field name as a TokenStream
fn get_field_name(property: &Property, index: usize, is_replica_tuple_struct: bool) -> Member {
    if is_replica_tuple_struct {
        let index = Index {
            index: index as u32,
            span: property.variable_name().span(),
        };
        Member::from(index)
    } else {
        Member::from(property.variable_name().clone())
    }
}

impl Property {
    pub fn normal(variable_name: Ident, inner_type: Type, replicable_property_type: Type) -> Self {
        Self::Normal(NormalProperty {
            variable_name: variable_name.clone(),
            inner_type,
            uppercase_variable_name: Ident::new(
                variable_name.to_string().to_uppercase().as_str(),
                Span::call_site(),
            ),
            replicable_property_type: replicable_property_type,
        })
    }

    pub fn entity(variable_name: Ident, replicable_entity_property_type: Type) -> Self {
        Self::Entity(EntityProperty {
            variable_name: variable_name.clone(),
            uppercase_variable_name: Ident::new(
                variable_name.to_string().to_uppercase().as_str(),
                Span::call_site(),
            ),
            replicable_entity_property_type: replicable_entity_property_type,
        })
    }

    pub fn variable_name(&self) -> &Ident {
        match self {
            Self::Normal(property) => &property.variable_name,
            Self::Entity(property) => &property.variable_name,
        }
    }

    pub fn uppercase_variable_name(&self) -> &Ident {
        match self {
            Self::Normal(property) => &property.uppercase_variable_name,
            Self::Entity(property) => &property.uppercase_variable_name,
        }
    }
}


/// Add the replicable properties
/// (either Property<T>, EntityProperty, or a Container<EntityProperty>)
fn properties(input: &DeriveInput) -> Vec<Property> {
    let mut fields = Vec::new();

    let mut add_fields = |property_seg: &PathSegment, variable_name: &Ident| {
        let property_type = &property_seg.ident;
        // EntityProperty
        if property_type == "EntityProperty" {
            fields.push(Property::entity(
                variable_name.clone(),
                parse_str::<Type>("EntityProperty").unwrap()
            ));
        }
        // VecDequeEntityProperty
        else if property_type == "VecDequeEntityProperty" {
            fields.push(Property::entity(
                variable_name.clone(),
                parse_str::<Type>("VecDequeEntityProperty").unwrap()
            ));
        }
        // Property
        else if property_type == "Property" {
            if let PathArguments::AngleBracketed(angle_args) = &property_seg.arguments {
                if let Some(GenericArgument::Type(inner_type)) = angle_args.args.first() {
                    fields.push(Property::normal(
                        variable_name.clone(),
                        inner_type.clone(),
                        parse_str::<Type>("Property").unwrap()
                    ));
                }
            }
        }
    };

    if let Data::Struct(data_struct) = &input.data {
        match &data_struct.fields {
            Fields::Named(fields_named) => {
                for field in fields_named.named.iter() {
                    if let Some(variable_name) = &field.ident {
                        if let Type::Path(type_path) = &field.ty {
                            if let Some(property_seg) = type_path.path.segments.first() {
                                add_fields(property_seg, variable_name);
                            }
                        }
                    }
                }
            }
            Fields::Unnamed(fields_unnamed) => {
                for (index, field) in fields_unnamed.unnamed.iter().enumerate() {
                    if let Type::Path(type_path) = &field.ty {
                        if let Some(property_seg) = type_path.path.segments.first() {
                            let property_type = property_seg.ident.clone();
                            let variable_name = get_variable_name_for_unnamed_field(index, property_type.span());
                            add_fields(property_seg, &variable_name);
                        }
                    }
                }
            }
            Fields::Unit => {}
        }
    } else {
        panic!("Can only derive Replicate on a struct");
    }

    fields
}

/// Returns true if the struct to replicate is a tuple struct, returns false if it contains
/// named fields
fn is_replica_tuple_struct(input: &DeriveInput) -> bool {
    if let Data::Struct(data_struct) = &input.data {
        return match &data_struct.fields {
            Fields::Named(_) => false,
            _ => true,
        };
    }
    false
}

fn protocol_path(input: &DeriveInput) -> (Path, Ident) {
    let mut path_result: Option<Result<Path>> = None;

    let attrs = &input.attrs;
    for option in attrs {
        let option = option.parse_meta().unwrap();
        if let Meta::NameValue(meta_name_value) = option {
            let path = meta_name_value.path;
            let lit = meta_name_value.lit;
            if let Some(ident) = path.get_ident() {
                if ident == "protocol_path" {
                    if let Lit::Str(lit_str) = lit {
                        path_result = Some(lit_str.parse());
                    }
                }
            }
        }
    }

    if let Some(Ok(path)) = path_result {
        let mut new_path = path;
        if let Some(last_seg) = new_path.segments.pop() {
            let name = last_seg.into_value().ident;
            if let Some(second_seg) = new_path.segments.pop() {
                new_path.segments.push_value(second_seg.into_value());
                return (new_path, name);
            }
        }
    }

    panic!("When deriving 'Replicate' you MUST specify the path of the accompanying protocol. IE: '#[protocol_path = \"crate::MyProtocol\"]'");
}

fn property_enum(enum_name: &Ident, properties: &[Property]) -> TokenStream {
    if properties.is_empty() {
        return quote! {
            enum #enum_name {}
        };
    }

    let hashtag = Punct::new('#', Spacing::Alone);

    let mut variant_list = quote! {};

    for (index, property) in properties.iter().enumerate() {
        let index = syn::Index::from(index);
        let uppercase_variant_name = property.uppercase_variable_name();

        let new_output_right = quote! {
            #uppercase_variant_name = #index as u8,
        };
        let new_output_result = quote! {
            #variant_list
            #new_output_right
        };
        variant_list = new_output_result;
    }

    quote! {
        #hashtag[repr(u8)]
        enum #enum_name {
            #variant_list
        }
    }
}

fn protocol_copy_method(protocol_name: &Ident, replica_name: &Ident) -> TokenStream {
    quote! {
        fn protocol_copy(&self) -> #protocol_name {
            return #protocol_name::#replica_name(self.clone());
        }
    }
}

fn into_protocol_method(protocol_name: &Ident, replica_name: &Ident) -> TokenStream {
    quote! {
        fn into_protocol(self) -> #protocol_name {
            return #protocol_name::#replica_name(self);
        }
    }
}

pub fn dyn_ref_method(protocol_name: &Ident) -> TokenStream {
    quote! {
        fn dyn_ref(&self) -> ReplicaDynRef<'_, #protocol_name> {
            return ReplicaDynRef::new(self);
        }
    }
}

pub fn dyn_mut_method(protocol_name: &Ident) -> TokenStream {
    quote! {
        fn dyn_mut(&mut self) -> ReplicaDynMut<'_, #protocol_name> {
            return ReplicaDynMut::new(self);
        }
    }
}

fn clone_method(
    replica_name: &Ident,
    properties: &[Property],
    is_replica_tuple_struct: bool,
) -> TokenStream {
    let mut output = quote! {};
    let mut entity_property_output = quote! {};

    for (index, property) in properties.iter().enumerate() {
        let field_name = get_field_name(property, index, is_replica_tuple_struct);
        match property {
            Property::Normal(_) => {
                let new_output_right = quote! {
                    (*self.#field_name).clone(),
                };
                let new_output_result = quote! {
                    #output
                    #new_output_right
                };
                output = new_output_result;
            }
            Property::Entity(_) => {
                let new_output_right = quote! {
                    new_clone.#field_name.mirror(&self.#field_name);
                };
                let new_output_result = quote! {
                    #entity_property_output
                    #new_output_right
                };
                entity_property_output = new_output_result;
            }
        };
    }

    quote! {
        fn clone(&self) -> #replica_name {
            let mut new_clone = #replica_name::new_complete(#output);
            #entity_property_output
            return new_clone;
        }
    }
}

fn mirror_method(
    protocol_name: &Ident,
    replica_name: &Ident,
    properties: &[Property],
    is_replica_tuple_struct: bool,
) -> TokenStream {
    let mut output = quote! {};

    for (index, property) in properties.iter().enumerate() {
        let field_name = get_field_name(property, index, is_replica_tuple_struct);
        let new_output_right = quote! {
            self.#field_name.mirror(&replica.#field_name);
        };
        let new_output_result = quote! {
            #output
            #new_output_right
        };
        output = new_output_result;
    }

    quote! {
        fn mirror(&mut self, other: &#protocol_name) {
            if let #protocol_name::#replica_name(replica) = other {
                #output
            }
        }
    }
}

fn set_mutator_method(properties: &[Property], is_replica_tuple_struct: bool) -> TokenStream {
    let mut output = quote! {};

    for (index, property) in properties.iter().enumerate() {
        let field_name = get_field_name(property, index, is_replica_tuple_struct);
        let new_output_right = quote! {
                self.#field_name.set_mutator(mutator);
        };
        let new_output_result = quote! {
            #output
            #new_output_right
        };
        output = new_output_result;
    }

    quote! {
        fn set_mutator(&mut self, mutator: &PropertyMutator) {
            #output
        }
    }
}

pub fn new_complete_method(
    replica_name: &Ident,
    enum_name: &Ident,
    properties: &[Property],
    is_replica_tuple_struct: bool,
) -> TokenStream {
    let mut args = quote! {};
    for property in properties.iter() {
        match property {
            Property::Normal(property) => {
                let field_name = &property.variable_name;
                let field_type = &property.inner_type;

                let new_output_right = quote! {
                    #field_name: #field_type,
                };

                let new_output_result = quote! {
                    #args #new_output_right
                };
                args = new_output_result;
            }
            Property::Entity(_) => {
                continue;
            }
        };
    }

    let mut fields = quote! {};
    for property in properties.iter() {
        let new_output_right = match property {
            Property::Normal(property) => {
                let field_name = &property.variable_name;
                let field_type = &property.inner_type;
                let replicable_property_type = &property.replicable_property_type;
                let uppercase_variant_name = &property.uppercase_variable_name;
                if is_replica_tuple_struct {
                    quote! {
                        <#replicable_property_type<#field_type>>::new(#field_name, #enum_name::#uppercase_variant_name as u8)
                    }
                } else {
                    quote! {
                        #field_name: <#replicable_property_type<#field_type>>::new(#field_name, #enum_name::#uppercase_variant_name as u8)
                    }
                }
            }
            Property::Entity(property) => {
                let field_name = &property.variable_name;
                let replicable_entity_property_type = &property.replicable_entity_property_type;
                let uppercase_variant_name = &property.uppercase_variable_name;
                if is_replica_tuple_struct {
                    quote! {
                        <#replicable_entity_property_type>::new(#enum_name::#uppercase_variant_name as u8)
                    }
                } else {
                    quote! {
                        #field_name: <#replicable_entity_property_type>::new(#enum_name::#uppercase_variant_name as u8)
                    }
                }
            }
        };

        let new_output_result = quote! {
            #fields
            #new_output_right,
        };
        fields = new_output_result;
    }

    let fn_inner = if is_replica_tuple_struct {
        quote! {
            #replica_name (
                #fields
            )
        }
    } else {
        quote! {
            #replica_name {
                #fields
            }
        }
    };

    quote! {
        pub fn new_complete(#args) -> #replica_name {
            #fn_inner
        }
    }
}

pub fn read_method(
    protocol_name: &Ident,
    replica_name: &Ident,
    enum_name: &Ident,
    properties: &[Property],
    is_replica_tuple_struct: bool,
) -> TokenStream {
    let mut prop_names = quote! {};
    for property in properties.iter() {
        let field_name = property.variable_name();
        let new_output_right = quote! {
            #field_name
        };
        let new_output_result = quote! {
            #prop_names
            #new_output_right,
        };
        prop_names = new_output_result;
    }

    let mut prop_reads = quote! {};
    for property in properties.iter() {
        let field_name = property.variable_name();
        let new_output_right = match property {
            Property::Normal(property) => {
                let replicable_property_type = &property.replicable_property_type;
                let field_type = &property.inner_type;
                let uppercase_variant_name = &property.uppercase_variable_name;
                quote! {
                    let #field_name = <#replicable_property_type<#field_type>>::new_read(reader, #enum_name::#uppercase_variant_name as u8)?;
                }
            }
            Property::Entity(property) => {
                let replicable_entity_property_type = &property.replicable_entity_property_type;
                let uppercase_variant_name = &property.uppercase_variable_name;
                quote! {
                    let #field_name = <#replicable_entity_property_type>::new_read(reader, #enum_name::#uppercase_variant_name as u8, converter)?;
                }
            }
        };

        let new_output_result = quote! {
            #prop_reads
            #new_output_right
        };
        prop_reads = new_output_result;
    }

    let replica_build = if is_replica_tuple_struct {
        quote! (
            #replica_name (
                #prop_names
            )
        )
    } else {
        quote! (
            #replica_name {
                #prop_names
            }
        )
    };

    quote! {
        pub fn read(reader: &mut BitReader, converter: &dyn NetEntityHandleConverter) -> Result<#protocol_name, SerdeErr> {
            #prop_reads

            return Ok(#protocol_name::#replica_name(#replica_build));
        }
    }
}

pub fn read_create_update_method(
    replica_name: &Ident,
    kind_name: &Ident,
    properties: &[Property],
) -> TokenStream {
    let mut prop_read_writes = quote! {};
    for property in properties.iter() {
        let new_output_right = match property {
            Property::Normal(property) => {
                let replicable_property_type = &property.replicable_property_type;
                let field_type = &property.inner_type;
                quote! {
                    {
                        let should_read = bool::de(reader)?;
                        should_read.ser(&mut update_writer);
                        if should_read {
                            <#replicable_property_type<#field_type>>::read_write(reader, &mut update_writer)?;
                        }
                    }
                }
            }
            Property::Entity(property) => {
                let replicable_entity_property_type = &property.replicable_entity_property_type;
                quote! {
                    {
                        let should_read = bool::de(reader)?;
                        should_read.ser(&mut update_writer);
                        if should_read {
                            <#replicable_entity_property_type>::read_write(reader, &mut update_writer)?;
                        }
                    }
                }
            }
        };

        let new_output_result = quote! {
            #prop_read_writes
            #new_output_right
        };
        prop_read_writes = new_output_result;
    }

    quote! {
        pub fn read_create_update(reader: &mut BitReader) -> Result<ComponentUpdate::<#kind_name>, SerdeErr> {

            let mut update_writer = BitWriter::new();

            #prop_read_writes

            let (length, buffer) = update_writer.flush();
            let owned_reader = OwnedBitReader::new(&buffer[..length]);

            return Ok(ComponentUpdate::new(#kind_name::#replica_name, owned_reader));
        }
    }
}

fn read_apply_update_method(
    kind_name: &Ident,
    properties: &[Property],
    is_replica_tuple_struct: bool,
) -> TokenStream {
    let mut output = quote! {};

    for (index, property) in properties.iter().enumerate() {
        let field_name = get_field_name(property, index, is_replica_tuple_struct);
        let new_output_right = match property {
            Property::Normal(property) => {
                let replicable_property_type = &property.replicable_property_type;
                quote! {
                    if bool::de(reader)? {
                        #replicable_property_type::read(&mut self.#field_name, reader)?;
                    }
                }
            }
            Property::Entity(property) => {
                let replicable_entity_property_type = &property.replicable_entity_property_type;
                quote! {
                    if bool::de(reader)? {
                        <#replicable_entity_property_type>::read(&mut self.#field_name, reader, converter)?;
                    }
                }
            }
        };

        let new_output_result = quote! {
            #output
            #new_output_right
        };
        output = new_output_result;
    }

    quote! {
        fn read_apply_update(&mut self, converter: &dyn NetEntityHandleConverter, mut update: ComponentUpdate<#kind_name>) -> Result<(), SerdeErr> {
            let reader = &mut update.reader();
            #output
            Ok(())
        }
    }
}

fn write_method(properties: &[Property], is_replica_tuple_struct: bool) -> TokenStream {
    let mut property_writes = quote! {};

    for (index, property) in properties.iter().enumerate() {
        let field_name = get_field_name(property, index, is_replica_tuple_struct);
        let new_output_right = match property {
            Property::Normal(property) => {
                let replicable_property_type = &property.replicable_property_type;
                quote! {
                    #replicable_property_type::write(&self.#field_name, bit_writer);
                }
            }
            Property::Entity(property) => {
                let replicable_entity_property_type = &property.replicable_entity_property_type;
                quote! {
                    <#replicable_entity_property_type>::write(&self.#field_name, bit_writer, converter);
                }
            }
        };

        let new_output_result = quote! {
            #property_writes
            #new_output_right
        };
        property_writes = new_output_result;
    }

    quote! {
        fn write(&self, bit_writer: &mut dyn BitWrite, converter: &dyn NetEntityHandleConverter) {
            self.kind().ser(bit_writer);
            #property_writes
        }
    }
}

fn write_update_method(
    enum_name: &Ident,
    properties: &[Property],
    is_replica_tuple_struct: bool,
) -> TokenStream {
    let mut output = quote! {};

    for (index, property) in properties.iter().enumerate() {
        let field_name = get_field_name(property, index, is_replica_tuple_struct);
        let new_output_right = match property {
            Property::Normal(property) => {
                let replicable_property_type = &property.replicable_property_type;
                let uppercase_variant_name = &property.uppercase_variable_name;
                quote! {
                    if let Some(true) = diff_mask.bit(#enum_name::#uppercase_variant_name as u8) {
                        true.ser(writer);
                        #replicable_property_type::write(&self.#field_name, writer);
                    } else {
                        false.ser(writer);
                    }
                }
            }
            Property::Entity(property) => {
                let replicable_entity_property_type = &property.replicable_entity_property_type;
                let uppercase_variant_name = &property.uppercase_variable_name;
                quote! {
                    if let Some(true) = diff_mask.bit(#enum_name::#uppercase_variant_name as u8) {
                        true.ser(writer);
                        <#replicable_entity_property_type>::write(&self.#field_name, writer, converter);
                    } else {
                        false.ser(writer);
                    }
                }
            }
        };

        let new_output_result = quote! {
            #output
            #new_output_right
        };
        output = new_output_result;
    }

    quote! {
        fn write_update(&self, diff_mask: &DiffMask, writer: &mut dyn BitWrite, converter: &dyn NetEntityHandleConverter) {
            #output
        }
    }
}

fn has_entity_properties_method(properties: &[Property]) -> TokenStream {
    for property in properties.iter() {
        if let Property::Entity(_) = property {
            return quote! {
                fn has_entity_properties(&self) -> bool {
                    return true;
                }
            };
        }
    }

    quote! {
        fn has_entity_properties(&self) -> bool {
            return false;
        }
    }
}

fn entities_method(properties: &[Property], is_replica_tuple_struct: bool) -> TokenStream {
    let mut body = quote! {};

    for (index, property) in properties.iter().enumerate() {
        if let Property::Entity(_) = property {
            let field_name = get_field_name(property, index, is_replica_tuple_struct);
            let body_add_right = quote! {
                output.extend(self.#field_name.entities());
            };
            let new_body = quote! {
                #body
                #body_add_right
            };
            body = new_body;
        }
    }

    quote! {
        fn entities(&self) -> Vec<internal::EntityHandle> {
            let mut output = Vec::new();
            #body
            return output;
        }
    }
}
