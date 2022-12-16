use std::any::Any;
use std::fmt::{Debug, Display, Formatter};
use std::ops::{Deref, DerefMut, Add, AddAssign, Mul, MulAssign};

use bevy_reflect::{Reflect, ReflectMut, ReflectOwned, ReflectRef, TypeInfo};
use naia_serde::{BitReader, BitWrite, BitWriter, Serde, SerdeErr};

use crate::protocol::property_mutate::PropertyMutator;

/// A Property of an Component/Message, that contains data
/// which must be tracked for updates
#[derive(Clone)]
pub struct Property<T: Serde> {
    inner: T,
    mutator: Option<PropertyMutator>,
    mutator_index: u8,
}


impl<T: Serde> Add for Property<T>
where T: Add<Output = T> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        let res = self.clone();
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
        let res = self.clone();
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
impl<T: Serde> Default for Property<T>
where T: Default {
    fn default() -> Self {
        Self::new(T::default(), 0)
    }
}

impl<T: Serde> Reflect for Property<T>
where T: Reflect {
    fn type_name(&self) -> &str {
        self.inner.type_name()
    }

    fn get_type_info(&self) -> &'static TypeInfo {
        self.inner.get_type_info()
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        Box::new(self.inner).into_any()
    }

    fn as_any(&self) -> &dyn Any {
        self.inner.as_any()
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self.inner.as_any_mut()
    }

    fn into_reflect(self: Box<Self>) -> Box<dyn Reflect> {
        Box::new(self.inner).into_reflect()
    }

    fn as_reflect(&self) -> &dyn Reflect {
        self.inner.as_reflect()
    }

    fn as_reflect_mut(&mut self) -> &mut dyn Reflect {
        self.inner.as_reflect_mut()
    }

    fn apply(&mut self, value: &dyn Reflect) {
        self.inner.apply(value)
    }

    fn set(&mut self, value: Box<dyn Reflect>) -> Result<(), Box<dyn Reflect>> {
        self.inner.set(value)
    }

    fn reflect_ref(&self) -> ReflectRef {
        self.inner.reflect_ref()
    }

    fn reflect_mut(&mut self) -> ReflectMut {
        self.inner.reflect_mut()
    }

    fn reflect_owned(self: Box<Self>) -> ReflectOwned {
        Box::new(self.inner).reflect_owned()
    }

    fn clone_value(&self) -> Box<dyn Reflect> {
        self.inner.clone_value()
    }
}

// should be shared
impl<T: Serde> Property<T> {
    /// Create a new Property
    pub fn new(value: T, mutator_index: u8) -> Property<T> {
        Property::<T> {
            inner: value,
            mutator: None,
            mutator_index,
        }
    }

    /// Set value to the value of another Property, queues for update if value
    /// changes
    pub fn mirror(&mut self, other: &Property<T>) {
        **self = (**other).clone();
    }

    // Serialization / deserialization

    /// Writes contained value into outgoing byte stream
    pub fn write(&self, writer: &mut dyn BitWrite) {
        self.inner.ser(writer);
    }

    /// Given a cursor into incoming packet data, initializes the Property with
    /// the synced value
    pub fn new_read(reader: &mut BitReader, mutator_index: u8) -> Result<Self, SerdeErr> {
        let inner = Self::read_inner(reader)?;

        Ok(Property::<T> {
            inner,
            mutator: None,
            mutator_index,
        })
    }

    /// Reads from a stream and immediately writes to a stream
    /// Used to buffer updates for later
    pub fn read_write(reader: &mut BitReader, writer: &mut BitWriter) -> Result<(), SerdeErr> {
        T::de(reader)?.ser(writer);
        Ok(())
    }

    /// Given a cursor into incoming packet data, updates the Property with the
    /// synced value
    pub fn read(&mut self, reader: &mut BitReader) -> Result<(), SerdeErr> {
        self.inner = Self::read_inner(reader)?;
        Ok(())
    }

    fn read_inner(reader: &mut BitReader) -> Result<T, SerdeErr> {
        T::de(reader)
    }

    // Comparison

    /// Compare to another property
    pub fn equals(&self, other: &Property<T>) -> bool {
        self.inner == other.inner
    }

    // Internal

    /// Set an PropertyMutator to track changes to the Property
    pub fn set_mutator(&mut self, mutator: &PropertyMutator) {
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
