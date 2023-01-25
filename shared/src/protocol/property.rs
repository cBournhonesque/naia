use std::fmt::{Debug, Display, Formatter};
use std::ops::{Deref, DerefMut, Add, AddAssign, Mul, MulAssign, Sub};


use ::serde::{Serialize, Deserialize, Serializer, Deserializer};
use naia_serde::{BitReader, BitWrite, BitWriter, Serde, SerdeErr};

use crate::protocol::property_mutate::PropertyMutator;
use crate::protocol::replicable_property::ReplicableProperty;

/// A Property of an Component/Message, that contains data
/// which must be tracked for updates
// #[cfg_attr(feature = "bevy_support", derive(Reflect))]
#[derive(Clone)]
pub struct Property<T: Serde> {
    inner: T,
    mutator: Option<PropertyMutator>,
    mutator_index: u8,
}

impl<T: Serde> PartialEq for Property<T> where T: PartialEq {
    fn eq(&self, other: &Self) -> bool {
        self.inner.eq(&other.inner)
    }
}

impl<T: Serde> Serialize for Property<T> where T: Serialize {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        self.inner.serialize(serializer)
    }
}


/// Deserialize the property according to the underlying field's deserializer
/// Again, same issues as with Default, we had to use 0 as mutator index
impl<'de, T: Serde> Deserialize<'de> for Property<T> where T: Deserialize<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
        let inner = T::deserialize(deserializer)?;
        Ok(Self::new(inner, 0))
    }
}


impl<T: Serde> Sub for Property<T>
    where T: Sub<Output = T> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        let mut res = self.clone();
        *res = self.inner - rhs.inner;
        res
    }
}


impl<T: Serde> Add for Property<T>
where T: Add<Output = T> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        let mut res = self.clone();
        *res = self.inner + rhs.inner;
        res
    }
}

impl<T: Serde> AddAssign for Property<T>
    where T: AddAssign {

    fn add_assign(&mut self, rhs: Self) {
        *self.deref_mut() += rhs.inner;
    }
}

impl<T: Serde> Mul<f32> for Property<T>
    where T: Mul<f32, Output = T> {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        let mut res = self.clone();
        *res = self.inner * rhs;
        res
    }
}

impl<T: Serde> MulAssign<f32> for Property<T>
    where T: MulAssign<f32> {

    fn mul_assign(&mut self, rhs: f32) {
        *self.deref_mut() *= rhs;
    }
}

impl<T: Serde> Debug for Property<T>
where T: Debug {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl<T: Serde> Display for Property<T>
where T: Display {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

/// Implement default for Property<T>
/// Note that this is invalid and shouldn't be used because the mutator_index is arbitrarily set to 0
/// It's mostly useful for bevy_inspector_egui
/// TODO: instead, impl Default on the replicate struct, and use new(default(), default(), etc.) or new_complete
///  so we dont need to implement default here
impl<T: Serde> Default for Property<T>
where T: Default {
    fn default() -> Self {
        Self::new(T::default(), 0)
    }
}


cfg_if! {
    if #[cfg(feature = "bevy_support")]
    {
        use std::any::Any;
        use bevy_reflect::{FromReflect, Reflect, ReflectMut, ReflectOwned, ReflectRef, TypeInfo};
        use bevy_reflect::{GetTypeRegistration, TypeRegistration};

        #[allow(unused_mut)]
        impl<T: Serde + Reflect> bevy_reflect::GetTypeRegistration for Property<T> {
            fn get_type_registration() -> bevy_reflect::TypeRegistration {
                let mut registration = bevy_reflect::TypeRegistration::of::<Property<T>>();
                registration
                    .insert::<
                        bevy_reflect::ReflectFromPtr,
                    >(bevy_reflect::FromType::<Property<T>>::from_type());
                let ignored_indices = [].into_iter();
                registration
                    .insert::<
                        bevy_reflect::serde::SerializationData,
                    >(bevy_reflect::serde::SerializationData::new(ignored_indices));
                registration
            }
        }
        impl<T: Serde + Reflect> bevy_reflect::Typed for Property<T> {
            fn type_info() -> &'static bevy_reflect::TypeInfo {
                static CELL: bevy_reflect::utility::GenericTypeInfoCell = bevy_reflect::utility::GenericTypeInfoCell::new();
                CELL.get_or_insert::<
                        Self,
                        _,
                    >(|| {
                    let fields = [
                        bevy_reflect::NamedField::new::<T>("inner"),
                        bevy_reflect::NamedField::new::<Option<PropertyMutator>>("mutator"),
                        bevy_reflect::NamedField::new::<u8>("mutator_index"),
                    ];
                    let info = bevy_reflect::StructInfo::new::<Self>("Property", &fields);
                    bevy_reflect::TypeInfo::Struct(info)
                })
            }
        }
        impl<T: Serde + Reflect> bevy_reflect::Struct for Property<T> {
            fn field(&self, name: &str) -> Option<&dyn bevy_reflect::Reflect> {
                match name {
                    "inner" => Some(&self.inner),
                    "mutator" => Some(&self.mutator),
                    "mutator_index" => Some(&self.mutator_index),
                    _ => None,
                }
            }
            fn field_mut(&mut self, name: &str) -> Option<&mut dyn bevy_reflect::Reflect> {
                match name {
                    "inner" => Some(&mut self.inner),
                    "mutator" => Some(&mut self.mutator),
                    "mutator_index" => Some(&mut self.mutator_index),
                    _ => None,
                }
            }
            fn field_at(&self, index: usize) -> Option<&dyn bevy_reflect::Reflect> {
                match index {
                    0usize => Some(&self.inner),
                    1usize => Some(&self.mutator),
                    2usize => Some(&self.mutator_index),
                    _ => None,
                }
            }
            fn field_at_mut(
                &mut self,
                index: usize,
            ) -> Option<&mut dyn bevy_reflect::Reflect> {
                match index {
                    0usize => Some(&mut self.inner),
                    1usize => Some(&mut self.mutator),
                    2usize => Some(&mut self.mutator_index),
                    _ => None,
                }
            }
            fn name_at(&self, index: usize) -> Option<&str> {
                match index {
                    0usize => Some("inner"),
                    1usize => Some("mutator"),
                    2usize => Some("mutator_index"),
                    _ => None,
                }
            }
            fn field_len(&self) -> usize {
                3usize
            }
            fn iter_fields(&self) -> bevy_reflect::FieldIter {
                bevy_reflect::FieldIter::new(self)
            }
            fn clone_dynamic(&self) -> bevy_reflect::DynamicStruct {
                let mut dynamic = bevy_reflect::DynamicStruct::default();
                dynamic.set_name(self.type_name().to_string());
                dynamic.insert_boxed("inner", self.inner.clone_value());
                dynamic.insert_boxed("mutator", self.mutator.clone_value());
                dynamic.insert_boxed("mutator_index", self.mutator_index.clone_value());
                dynamic
            }
        }
        impl<T: Serde + Reflect> bevy_reflect::Reflect for Property<T> {
            #[inline]
            fn type_name(&self) -> &str {
                std::any::type_name::<Self>()
            }
            #[inline]
            fn get_type_info(&self) -> &'static bevy_reflect::TypeInfo {
                <Self as bevy_reflect::Typed>::type_info()
            }
            #[inline]
            fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
                self
            }
            #[inline]
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            #[inline]
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
            #[inline]
            fn into_reflect(self: Box<Self>) -> Box<dyn bevy_reflect::Reflect> {
                self
            }
            #[inline]
            fn as_reflect(&self) -> &dyn bevy_reflect::Reflect {
                self
            }
            #[inline]
            fn as_reflect_mut(&mut self) -> &mut dyn bevy_reflect::Reflect {
                self
            }
            #[inline]
            fn clone_value(&self) -> Box<dyn bevy_reflect::Reflect> {
                Box::new(bevy_reflect::Struct::clone_dynamic(self))
            }
            #[inline]
            fn set(
                &mut self,
                value: Box<dyn bevy_reflect::Reflect>,
            ) -> Result<(), Box<dyn bevy_reflect::Reflect>> {
                *self = value.take()?;
                Ok(())
            }
            #[inline]
            fn apply(&mut self, value: &dyn bevy_reflect::Reflect) {
                if let bevy_reflect::ReflectRef::Struct(struct_value) = value.reflect_ref() {
                    for (i, value) in struct_value.iter_fields().enumerate() {
                        let name = struct_value.name_at(i).unwrap();
                        bevy_reflect::Struct::field_mut(self, name).map(|v| v.apply(value));
                    }
                } else {
                    ::core::panicking::panic_fmt(
                        ::core::fmt::Arguments::new_v1(
                            &["Attempted to apply non-struct type to struct type."],
                            &[],
                        ),
                    );
                }
            }
            fn reflect_ref(&self) -> bevy_reflect::ReflectRef {
                bevy_reflect::ReflectRef::Struct(self)
            }
            fn reflect_mut(&mut self) -> bevy_reflect::ReflectMut {
                bevy_reflect::ReflectMut::Struct(self)
            }
            fn reflect_owned(self: Box<Self>) -> bevy_reflect::ReflectOwned {
                bevy_reflect::ReflectOwned::Struct(self)
            }
            fn reflect_partial_eq(&self, value: &dyn bevy_reflect::Reflect) -> Option<bool> {
                bevy_reflect::struct_partial_eq(self, value)
            }
        }

    }
}

// cfg_if! {
//     if #[cfg(feature = "bevy_support")]
//     {
//         use std::any::Any;
//         use bevy_reflect::{FromReflect, Reflect, ReflectMut, ReflectOwned, ReflectRef, TypeInfo};
//         use bevy_reflect::{GetTypeRegistration, TypeRegistration};
//
//         impl<T: Serde> GetTypeRegistration for Property<T> where T: GetTypeRegistration {
//             fn get_type_registration() -> TypeRegistration {
//                 T::get_type_registration()
//             }
//         }
//
//         impl<T: Serde> Reflect for Property<T> where T: Reflect {
//             fn type_name(&self) -> &str {
//                 self.inner.type_name()
//             }
//
//             fn get_type_info(&self) -> &'static TypeInfo {
//                 self.inner.get_type_info()
//             }
//
//             fn into_any(self: Box<Self>) -> Box<dyn Any> {
//                 Box::new(self.inner).into_any()
//             }
//
//             fn as_any(&self) -> &dyn Any {
//                 self.inner.as_any()
//             }
//
//             fn as_any_mut(&mut self) -> &mut dyn Any {
//                 self.inner.as_any_mut()
//             }
//
//             fn into_reflect(self: Box<Self>) -> Box<dyn Reflect> {
//                 Box::new(self.inner).into_reflect()
//             }
//
//             fn as_reflect(&self) -> &dyn Reflect {
//                 self.inner.as_reflect()
//             }
//
//             fn as_reflect_mut(&mut self) -> &mut dyn Reflect {
//                 self.inner.as_reflect_mut()
//             }
//
//             fn apply(&mut self, value: &dyn Reflect) {
//                 self.inner.apply(value)
//             }
//
//             fn set(&mut self, value: Box<dyn Reflect>) -> Result<(), Box<dyn Reflect>> {
//                 self.inner.set(value)
//             }
//
//             fn reflect_ref(&self) -> ReflectRef {
//                 self.inner.reflect_ref()
//             }
//
//             fn reflect_mut(&mut self) -> ReflectMut {
//                 self.inner.reflect_mut()
//             }
//
//             fn reflect_owned(self: Box<Self>) -> ReflectOwned {
//                 Box::new(self.inner).reflect_owned()
//             }
//
//             fn clone_value(&self) -> Box<dyn Reflect> {
//                 self.inner.clone_value()
//             }
//         }
//
//         impl<T: Serde> FromReflect for Property<T> where T: FromReflect {
//
//             fn from_reflect(reflect: &dyn Reflect) -> Option<Self> {
//                 match T::from_reflect(reflect) {
//                     None => None,
//                     Some(inner) => Some(Self::new(inner, 0))
//                 }
//             }
//
//         }
//
//     }
//
// }


impl<T: Serde> Property<T> {
    fn read_inner(reader: &mut BitReader) -> Result<T, SerdeErr> {
        T::de(reader)
    }
}

// should be shared
impl<T: Serde> ReplicableProperty for Property<T> {
    type Inner = T;

    /// Create a new Property
    fn new(value: Self::Inner, mutator_index: u8) -> Self {
        Property::<T> {
            inner: value,
            mutator: None,
            mutator_index,
        }
    }

    /// Set value to the value of another Property, queues for update if value
    /// changes
    fn mirror(&mut self, other: &Self) {
        **self = (**other).clone();
    }

    // Serialization / deserialization

    /// Writes contained value into outgoing byte stream
    fn write(&self, writer: &mut dyn BitWrite) {
        self.inner.ser(writer);
    }

    /// Given a cursor into incoming packet data, initializes the Property with
    /// the synced value
    fn new_read(reader: &mut BitReader, mutator_index: u8) -> Result<Self, SerdeErr> {
        let inner = Self::read_inner(reader)?;

        Ok(Property::<T> {
            inner,
            mutator: None,
            mutator_index,
        })
    }

    /// Reads from a stream and immediately writes to a stream
    /// Used to buffer updates for later
    fn read_write(reader: &mut BitReader, writer: &mut BitWriter) -> Result<(), SerdeErr> {
        T::de(reader)?.ser(writer);
        Ok(())
    }

    /// Given a cursor into incoming packet data, updates the Property with the
    /// synced value
    fn read(&mut self, reader: &mut BitReader) -> Result<(), SerdeErr> {
        self.inner = Self::read_inner(reader)?;
        Ok(())
    }

    // Comparison

    /// Compare to another property
    fn equals(&self, other: &Self) -> bool {
        self.inner == other.inner
    }

    // Internal

    /// Set an PropertyMutator to track changes to the Property
    fn set_mutator(&mut self, mutator: &PropertyMutator) {
        self.mutator = Some(mutator.clone_new());
    }
}

// It could be argued that Property here is a type of smart-pointer,
// but honestly this is mainly for the convenience of type coercion
impl<T: Serde> Deref for Property<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: Serde> DerefMut for Property<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Just assume inner value will be changed, queue for update
        if let Some(mutator) = &mut self.mutator {
            mutator.mutate(self.mutator_index);
        }
        &mut self.inner
    }
}
