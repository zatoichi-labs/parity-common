// This file is part of Substrate.

// Copyright (C) 2017-2023 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Traits, types and structs to support a bounded BTreeMap.

use crate::{Get, TryCollect};
use alloc::collections::BTreeMap;
use core::{borrow::Borrow, marker::PhantomData, ops::Deref};
#[cfg(feature = "serde")]
use serde::{
	de::{Error, MapAccess, Visitor},
	Deserialize, Deserializer, Serialize,
};

/// A bounded map based on a B-Tree.
///
/// B-Trees represent a fundamental compromise between cache-efficiency and actually minimizing
/// the amount of work performed in a search. See [`BTreeMap`] for more details.
///
/// Unlike a standard `BTreeMap`, there is an enforced upper limit to the number of items in the
/// map. All internal operations ensure this bound is respected.
#[cfg_attr(feature = "serde", derive(Serialize), serde(transparent))]
#[cfg_attr(feature = "scale-codec", derive(scale_codec::Encode, scale_info::TypeInfo))]
#[cfg_attr(feature = "scale-codec", scale_info(skip_type_params(S)))]
#[cfg_attr(feature = "jam-codec", derive(jam_codec::Encode))]
pub struct BoundedBTreeMap<K, V, S>(
	BTreeMap<K, V>,
	#[cfg_attr(feature = "serde", serde(skip_serializing))] PhantomData<S>,
);

#[cfg(feature = "serde")]
impl<'de, K, V, S: Get<u32>> Deserialize<'de> for BoundedBTreeMap<K, V, S>
where
	K: Deserialize<'de> + Ord,
	V: Deserialize<'de>,
{
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		// Create a visitor to visit each element in the map
		struct BTreeMapVisitor<K, V, S>(PhantomData<(K, V, S)>);

		impl<'de, K, V, S> Visitor<'de> for BTreeMapVisitor<K, V, S>
		where
			K: Deserialize<'de> + Ord,
			V: Deserialize<'de>,
			S: Get<u32>,
		{
			type Value = BTreeMap<K, V>;

			fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
				formatter.write_str("a map")
			}

			fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
			where
				A: MapAccess<'de>,
			{
				let size = map.size_hint().unwrap_or(0);
				let max = S::get() as usize;
				if size > max {
					Err(A::Error::custom("map exceeds the size of the bounds"))
				} else {
					let mut values = BTreeMap::new();

					while let Some(key) = map.next_key()? {
						if values.len() >= max {
							return Err(A::Error::custom("map exceeds the size of the bounds"));
						}
						let value = map.next_value()?;
						values.insert(key, value);
					}

					Ok(values)
				}
			}
		}

		let visitor: BTreeMapVisitor<K, V, S> = BTreeMapVisitor(PhantomData);
		deserializer.deserialize_map(visitor).map(|v| {
			BoundedBTreeMap::<K, V, S>::try_from(v)
				.map_err(|_| Error::custom("failed to create a BoundedBTreeMap from the provided map"))
		})?
	}
}

impl<K, V, S> BoundedBTreeMap<K, V, S>
where
	S: Get<u32>,
{
	/// Get the bound of the type in `usize`.
	pub fn bound() -> usize {
		S::get() as usize
	}
}

impl<K, V, S> BoundedBTreeMap<K, V, S>
where
	K: Ord,
	S: Get<u32>,
{
	/// Create `Self` from `t` without any checks.
	fn unchecked_from(t: BTreeMap<K, V>) -> Self {
		Self(t, Default::default())
	}

	/// Exactly the same semantics as `BTreeMap::retain`.
	///
	/// The is a safe `&mut self` borrow because `retain` can only ever decrease the length of the
	/// inner map.
	pub fn retain<F: FnMut(&K, &mut V) -> bool>(&mut self, f: F) {
		self.0.retain(f)
	}

	/// Create a new `BoundedBTreeMap`.
	///
	/// Does not allocate.
	pub fn new() -> Self {
		BoundedBTreeMap(BTreeMap::new(), PhantomData)
	}

	/// Consume self, and return the inner `BTreeMap`.
	///
	/// This is useful when a mutating API of the inner type is desired, and closure-based mutation
	/// such as provided by [`try_mutate`][Self::try_mutate] is inconvenient.
	pub fn into_inner(self) -> BTreeMap<K, V> {
		debug_assert!(self.0.len() <= Self::bound());
		self.0
	}

	/// Consumes self and mutates self via the given `mutate` function.
	///
	/// If the outcome of mutation is within bounds, `Some(Self)` is returned. Else, `None` is
	/// returned.
	///
	/// This is essentially a *consuming* shorthand [`Self::into_inner`] -> `...` ->
	/// [`Self::try_from`].
	pub fn try_mutate(mut self, mut mutate: impl FnMut(&mut BTreeMap<K, V>)) -> Option<Self> {
		mutate(&mut self.0);
		(self.0.len() <= Self::bound()).then(move || self)
	}

	/// Clears the map, removing all elements.
	pub fn clear(&mut self) {
		self.0.clear()
	}

	/// Return a mutable reference to the value corresponding to the key.
	///
	/// The key may be any borrowed form of the map's key type, but the ordering on the borrowed
	/// form _must_ match the ordering on the key type.
	pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
	where
		K: Borrow<Q>,
		Q: Ord + ?Sized,
	{
		self.0.get_mut(key)
	}

	/// Exactly the same semantics as [`BTreeMap::insert`], but returns an `Err` (and is a noop) if
	/// the new length of the map exceeds `S`.
	///
	/// In the `Err` case, returns the inserted pair so it can be further used without cloning.
	pub fn try_insert(&mut self, key: K, value: V) -> Result<Option<V>, (K, V)> {
		if self.len() < Self::bound() || self.0.contains_key(&key) {
			Ok(self.0.insert(key, value))
		} else {
			Err((key, value))
		}
	}

	/// Remove a key from the map, returning the value at the key if the key was previously in the
	/// map.
	///
	/// The key may be any borrowed form of the map's key type, but the ordering on the borrowed
	/// form _must_ match the ordering on the key type.
	pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
	where
		K: Borrow<Q>,
		Q: Ord + ?Sized,
	{
		self.0.remove(key)
	}

	/// Remove a key from the map, returning the value at the key if the key was previously in the
	/// map.
	///
	/// The key may be any borrowed form of the map's key type, but the ordering on the borrowed
	/// form _must_ match the ordering on the key type.
	pub fn remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
	where
		K: Borrow<Q>,
		Q: Ord + ?Sized,
	{
		self.0.remove_entry(key)
	}

	/// Gets a mutable iterator over the entries of the map, sorted by key.
	///
	/// See [`BTreeMap::iter_mut`] for more information.
	pub fn iter_mut(&mut self) -> alloc::collections::btree_map::IterMut<K, V> {
		self.0.iter_mut()
	}

	/// Consume the map, applying `f` to each of it's values and returning a new map.
	pub fn map<T, F>(self, mut f: F) -> BoundedBTreeMap<K, T, S>
	where
		F: FnMut((&K, V)) -> T,
	{
		BoundedBTreeMap::<K, T, S>::unchecked_from(
			self.0
				.into_iter()
				.map(|(k, v)| {
					let t = f((&k, v));
					(k, t)
				})
				.collect(),
		)
	}

	/// Consume the map, applying `f` to each of it's values as long as it returns successfully. If
	/// an `Err(E)` is ever encountered, the mapping is short circuited and the error is returned;
	/// otherwise, a new map is returned in the contained `Ok` value.
	pub fn try_map<T, E, F>(self, mut f: F) -> Result<BoundedBTreeMap<K, T, S>, E>
	where
		F: FnMut((&K, V)) -> Result<T, E>,
	{
		Ok(BoundedBTreeMap::<K, T, S>::unchecked_from(
			self.0
				.into_iter()
				.map(|(k, v)| (f((&k, v)).map(|t| (k, t))))
				.collect::<Result<BTreeMap<_, _>, _>>()?,
		))
	}

	/// Returns true if this map is full.
	pub fn is_full(&self) -> bool {
		self.len() >= Self::bound()
	}
}

impl<K, V, S> Default for BoundedBTreeMap<K, V, S>
where
	K: Ord,
	S: Get<u32>,
{
	fn default() -> Self {
		Self::new()
	}
}

impl<K, V, S> Clone for BoundedBTreeMap<K, V, S>
where
	BTreeMap<K, V>: Clone,
{
	fn clone(&self) -> Self {
		BoundedBTreeMap(self.0.clone(), PhantomData)
	}
}

impl<K, V, S> core::fmt::Debug for BoundedBTreeMap<K, V, S>
where
	BTreeMap<K, V>: core::fmt::Debug,
	S: Get<u32>,
{
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		f.debug_tuple("BoundedBTreeMap").field(&self.0).field(&Self::bound()).finish()
	}
}

// Custom implementation of `Hash` since deriving it would require all generic bounds to also
// implement it.
#[cfg(feature = "std")]
impl<K: std::hash::Hash, V: std::hash::Hash, S> std::hash::Hash for BoundedBTreeMap<K, V, S> {
	fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
		self.0.hash(state);
	}
}

impl<K, V, S1, S2> PartialEq<BoundedBTreeMap<K, V, S1>> for BoundedBTreeMap<K, V, S2>
where
	BTreeMap<K, V>: PartialEq,
	S1: Get<u32>,
	S2: Get<u32>,
{
	fn eq(&self, other: &BoundedBTreeMap<K, V, S1>) -> bool {
		S1::get() == S2::get() && self.0 == other.0
	}
}

impl<K, V, S> Eq for BoundedBTreeMap<K, V, S>
where
	BTreeMap<K, V>: Eq,
	S: Get<u32>,
{
}

impl<K, V, S> PartialEq<BTreeMap<K, V>> for BoundedBTreeMap<K, V, S>
where
	BTreeMap<K, V>: PartialEq,
{
	fn eq(&self, other: &BTreeMap<K, V>) -> bool {
		self.0 == *other
	}
}

impl<K, V, S> PartialOrd for BoundedBTreeMap<K, V, S>
where
	BTreeMap<K, V>: PartialOrd,
	S: Get<u32>,
{
	fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
		self.0.partial_cmp(&other.0)
	}
}

impl<K, V, S> Ord for BoundedBTreeMap<K, V, S>
where
	BTreeMap<K, V>: Ord,
	S: Get<u32>,
{
	fn cmp(&self, other: &Self) -> core::cmp::Ordering {
		self.0.cmp(&other.0)
	}
}

impl<K, V, S> IntoIterator for BoundedBTreeMap<K, V, S> {
	type Item = (K, V);
	type IntoIter = alloc::collections::btree_map::IntoIter<K, V>;

	fn into_iter(self) -> Self::IntoIter {
		self.0.into_iter()
	}
}

impl<'a, K, V, S> IntoIterator for &'a BoundedBTreeMap<K, V, S> {
	type Item = (&'a K, &'a V);
	type IntoIter = alloc::collections::btree_map::Iter<'a, K, V>;

	fn into_iter(self) -> Self::IntoIter {
		self.0.iter()
	}
}

impl<'a, K, V, S> IntoIterator for &'a mut BoundedBTreeMap<K, V, S> {
	type Item = (&'a K, &'a mut V);
	type IntoIter = alloc::collections::btree_map::IterMut<'a, K, V>;

	fn into_iter(self) -> Self::IntoIter {
		self.0.iter_mut()
	}
}

impl<K, V, S> Deref for BoundedBTreeMap<K, V, S>
where
	K: Ord,
{
	type Target = BTreeMap<K, V>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl<K, V, S> AsRef<BTreeMap<K, V>> for BoundedBTreeMap<K, V, S>
where
	K: Ord,
{
	fn as_ref(&self) -> &BTreeMap<K, V> {
		&self.0
	}
}

impl<K, V, S> From<BoundedBTreeMap<K, V, S>> for BTreeMap<K, V>
where
	K: Ord,
{
	fn from(map: BoundedBTreeMap<K, V, S>) -> Self {
		map.0
	}
}

impl<K, V, S> TryFrom<BTreeMap<K, V>> for BoundedBTreeMap<K, V, S>
where
	K: Ord,
	S: Get<u32>,
{
	type Error = ();

	fn try_from(value: BTreeMap<K, V>) -> Result<Self, Self::Error> {
		(value.len() <= Self::bound())
			.then(move || BoundedBTreeMap(value, PhantomData))
			.ok_or(())
	}
}

impl<I, K, V, Bound> TryCollect<BoundedBTreeMap<K, V, Bound>> for I
where
	K: Ord,
	I: ExactSizeIterator + Iterator<Item = (K, V)>,
	Bound: Get<u32>,
{
	type Error = &'static str;

	fn try_collect(self) -> Result<BoundedBTreeMap<K, V, Bound>, Self::Error> {
		if self.len() > Bound::get() as usize {
			Err("iterator length too big")
		} else {
			Ok(BoundedBTreeMap::<K, V, Bound>::unchecked_from(self.collect::<BTreeMap<K, V>>()))
		}
	}
}

#[cfg(any(feature = "scale-codec", feature = "jam-codec"))]
macro_rules! codec_impl {
	($codec:ident) => {
		use super::*;
		use crate::codec_utils::PrependCompactInput;
		use $codec::{
			Compact, Decode, DecodeLength, DecodeWithMemTracking, Encode, EncodeLike, Error, Input, MaxEncodedLen,
		};

		impl<K, V, S> Decode for BoundedBTreeMap<K, V, S>
		where
			K: Decode + Ord,
			V: Decode,
			S: Get<u32>,
		{
			fn decode<I: Input>(input: &mut I) -> Result<Self, Error> {
				// Fail early if the len is too big. This is a compact u32 which we will later put back.
				let len = <Compact<u32>>::decode(input)?;
				if len.0 > S::get() {
					return Err("BoundedBTreeMap exceeds its limit".into());
				}
				// Reconstruct the original input by prepending the length we just read, then delegate the decoding to BTreeMap.
				let inner = BTreeMap::decode(&mut PrependCompactInput {
					encoded_len: len.encode().as_ref(),
					read: 0,
					inner: input,
				})?;
				Ok(Self(inner, PhantomData))
			}

			fn skip<I: Input>(input: &mut I) -> Result<(), Error> {
				BTreeMap::<K, V>::skip(input)
			}
		}

		impl<K, V, S> DecodeWithMemTracking for BoundedBTreeMap<K, V, S>
		where
			K: DecodeWithMemTracking + Ord,
			V: DecodeWithMemTracking,
			S: Get<u32>,
			BoundedBTreeMap<K, V, S>: Decode,
		{
		}

		impl<K, V, S> MaxEncodedLen for BoundedBTreeMap<K, V, S>
		where
			K: MaxEncodedLen,
			V: MaxEncodedLen,
			S: Get<u32>,
		{
			fn max_encoded_len() -> usize {
				Self::bound()
					.saturating_mul(K::max_encoded_len().saturating_add(V::max_encoded_len()))
					.saturating_add(Compact(S::get()).encoded_size())
			}
		}

		impl<K, V, S> EncodeLike<BTreeMap<K, V>> for BoundedBTreeMap<K, V, S> where BTreeMap<K, V>: Encode {}

		impl<K, V, S> DecodeLength for BoundedBTreeMap<K, V, S> {
			fn len(self_encoded: &[u8]) -> Result<usize, Error> {
				// `BoundedBTreeMap<K, V, S>` is stored just a `BTreeMap<K, V>`, which is stored as a
				// `Compact<u32>` with its length followed by an iteration of its items. We can just use
				// the underlying implementation.
				<BTreeMap<K, V> as DecodeLength>::len(self_encoded)
			}
		}
	};
}

#[cfg(feature = "scale-codec")]
mod scale_codec_impl {
	codec_impl!(scale_codec);
}

#[cfg(feature = "jam-codec")]
mod jam_codec_impl {
	codec_impl!(jam_codec);
}

#[cfg(test)]
mod test {
	use super::*;
	use crate::ConstU32;
	use alloc::{vec, vec::Vec};
	#[cfg(feature = "scale-codec")]
	use scale_codec::{Compact, CompactLen, Decode, Encode};

	fn map_from_keys<K>(keys: &[K]) -> BTreeMap<K, ()>
	where
		K: Ord + Copy,
	{
		keys.iter().copied().zip(core::iter::repeat(())).collect()
	}

	fn boundedmap_from_keys<K, S>(keys: &[K]) -> BoundedBTreeMap<K, (), S>
	where
		K: Ord + Copy,
		S: Get<u32>,
	{
		map_from_keys(keys).try_into().unwrap()
	}

	#[test]
	#[cfg(feature = "scale-codec")]
	fn encoding_same_as_unbounded_map() {
		let b = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2, 3, 4, 5, 6]);
		let m = map_from_keys(&[1, 2, 3, 4, 5, 6]);

		assert_eq!(b.encode(), m.encode());
	}

	#[test]
	#[cfg(feature = "scale-codec")]
	fn encode_then_decode_gives_original_map() {
		let b = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2, 3, 4, 5, 6]);
		let b_encode_decode = BoundedBTreeMap::<u32, (), ConstU32<7>>::decode(&mut &b.encode()[..]).unwrap();

		assert_eq!(b_encode_decode, b);
	}

	#[test]
	fn try_insert_works() {
		let mut bounded = boundedmap_from_keys::<u32, ConstU32<4>>(&[1, 2, 3]);
		bounded.try_insert(0, ()).unwrap();
		assert_eq!(*bounded, map_from_keys(&[1, 0, 2, 3]));

		assert!(bounded.try_insert(9, ()).is_err());
		assert_eq!(*bounded, map_from_keys(&[1, 0, 2, 3]));
	}

	#[test]
	fn deref_coercion_works() {
		let bounded = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2, 3]);
		// these methods come from deref-ed vec.
		assert_eq!(bounded.len(), 3);
		assert!(bounded.iter().next().is_some());
		assert!(!bounded.is_empty());
	}

	#[test]
	fn try_mutate_works() {
		let bounded = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2, 3, 4, 5, 6]);
		let bounded = bounded
			.try_mutate(|v| {
				v.insert(7, ());
			})
			.unwrap();
		assert_eq!(bounded.len(), 7);
		assert!(bounded
			.try_mutate(|v| {
				v.insert(8, ());
			})
			.is_none());
	}

	#[test]
	fn btree_map_eq_works() {
		let bounded = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2, 3, 4, 5, 6]);
		assert_eq!(bounded, map_from_keys(&[1, 2, 3, 4, 5, 6]));
	}

	#[test]
	#[cfg(feature = "scale-codec")]
	fn too_big_fail_to_decode() {
		let v: Vec<(u32, u32)> = vec![(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)];
		assert_eq!(
			BoundedBTreeMap::<u32, u32, ConstU32<4>>::decode(&mut &v.encode()[..]),
			Err("BoundedBTreeMap exceeds its limit".into()),
		);
	}

	#[test]
	#[cfg(feature = "scale-codec")]
	fn dont_consume_more_data_than_bounded_len() {
		let m = map_from_keys(&[1, 2, 3, 4, 5, 6]);
		let data = m.encode();
		let data_input = &mut &data[..];

		BoundedBTreeMap::<u32, u32, ConstU32<4>>::decode(data_input).unwrap_err();
		assert_eq!(data_input.len(), data.len() - Compact::<u32>::compact_len(&(data.len() as u32)));
	}

	#[test]
	fn unequal_eq_impl_insert_works() {
		// given a struct with a strange notion of equality
		#[derive(Debug)]
		struct Unequal(u32, bool);

		impl PartialEq for Unequal {
			fn eq(&self, other: &Self) -> bool {
				self.0 == other.0
			}
		}
		impl Eq for Unequal {}

		impl Ord for Unequal {
			fn cmp(&self, other: &Self) -> core::cmp::Ordering {
				self.0.cmp(&other.0)
			}
		}

		impl PartialOrd for Unequal {
			fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
				Some(self.cmp(other))
			}
		}

		let mut map = BoundedBTreeMap::<Unequal, u32, ConstU32<4>>::new();

		// when the set is full

		for i in 0..4 {
			map.try_insert(Unequal(i, false), i).unwrap();
		}

		// can't insert a new distinct member
		map.try_insert(Unequal(5, false), 5).unwrap_err();

		// but _can_ insert a distinct member which compares equal, though per the documentation,
		// neither the set length nor the actual member are changed, but the value is
		map.try_insert(Unequal(0, true), 6).unwrap();
		assert_eq!(map.len(), 4);
		let (zero_key, zero_value) = map.get_key_value(&Unequal(0, true)).unwrap();
		assert_eq!(zero_key.0, 0);
		assert_eq!(zero_key.1, false);
		assert_eq!(*zero_value, 6);
	}

	#[test]
	fn eq_works() {
		// of same type
		let b1 = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2]);
		let b2 = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2]);
		assert_eq!(b1, b2);

		// of different type, but same value and bound.
		crate::parameter_types! {
			B1: u32 = 7;
			B2: u32 = 7;
		}
		let b1 = boundedmap_from_keys::<u32, B1>(&[1, 2]);
		let b2 = boundedmap_from_keys::<u32, B2>(&[1, 2]);
		assert_eq!(b1, b2);
	}

	#[test]
	fn can_be_collected() {
		let b1 = boundedmap_from_keys::<u32, ConstU32<5>>(&[1, 2, 3, 4]);
		let b2: BoundedBTreeMap<u32, (), ConstU32<5>> = b1.iter().map(|(k, v)| (k + 1, *v)).try_collect().unwrap();
		assert_eq!(b2.into_iter().map(|(k, _)| k).collect::<Vec<_>>(), vec![2, 3, 4, 5]);

		// can also be collected into a collection of length 4.
		let b2: BoundedBTreeMap<u32, (), ConstU32<4>> = b1.iter().map(|(k, v)| (k + 1, *v)).try_collect().unwrap();
		assert_eq!(b2.into_iter().map(|(k, _)| k).collect::<Vec<_>>(), vec![2, 3, 4, 5]);

		// can be mutated further into iterators that are `ExactSizedIterator`.
		let b2: BoundedBTreeMap<u32, (), ConstU32<5>> =
			b1.iter().map(|(k, v)| (k + 1, *v)).rev().skip(2).try_collect().unwrap();
		// note that the binary tree will re-sort this, so rev() is not really seen
		assert_eq!(b2.into_iter().map(|(k, _)| k).collect::<Vec<_>>(), vec![2, 3]);

		let b2: BoundedBTreeMap<u32, (), ConstU32<5>> =
			b1.iter().map(|(k, v)| (k + 1, *v)).take(2).try_collect().unwrap();
		assert_eq!(b2.into_iter().map(|(k, _)| k).collect::<Vec<_>>(), vec![2, 3]);

		// but these won't work
		let b2: Result<BoundedBTreeMap<u32, (), ConstU32<3>>, _> = b1.iter().map(|(k, v)| (k + 1, *v)).try_collect();
		assert!(b2.is_err());

		let b2: Result<BoundedBTreeMap<u32, (), ConstU32<1>>, _> =
			b1.iter().map(|(k, v)| (k + 1, *v)).skip(2).try_collect();
		assert!(b2.is_err());
	}

	#[test]
	fn test_iter_mut() {
		let mut b1: BoundedBTreeMap<u8, u8, ConstU32<7>> =
			[1, 2, 3, 4].into_iter().map(|k| (k, k)).try_collect().unwrap();

		let b2: BoundedBTreeMap<u8, u8, ConstU32<7>> =
			[1, 2, 3, 4].into_iter().map(|k| (k, k * 2)).try_collect().unwrap();

		b1.iter_mut().for_each(|(_, v)| *v *= 2);

		assert_eq!(b1, b2);
	}

	#[test]
	fn map_retains_size() {
		let b1 = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2]);
		let b2 = b1.clone();

		assert_eq!(b1.len(), b2.map(|(_, _)| 5_u32).len());
	}

	#[test]
	fn map_maps_properly() {
		let b1: BoundedBTreeMap<u32, u32, ConstU32<7>> =
			[1, 2, 3, 4].into_iter().map(|k| (k, k * 2)).try_collect().unwrap();
		let b2: BoundedBTreeMap<u32, u32, ConstU32<7>> =
			[1, 2, 3, 4].into_iter().map(|k| (k, k)).try_collect().unwrap();

		assert_eq!(b1, b2.map(|(_, v)| v * 2));
	}

	#[test]
	fn try_map_retains_size() {
		let b1 = boundedmap_from_keys::<u32, ConstU32<7>>(&[1, 2]);
		let b2 = b1.clone();

		assert_eq!(b1.len(), b2.try_map::<_, (), _>(|(_, _)| Ok(5_u32)).unwrap().len());
	}

	#[test]
	fn try_map_maps_properly() {
		let b1: BoundedBTreeMap<u32, u32, ConstU32<7>> =
			[1, 2, 3, 4].into_iter().map(|k| (k, k * 2)).try_collect().unwrap();
		let b2: BoundedBTreeMap<u32, u32, ConstU32<7>> =
			[1, 2, 3, 4].into_iter().map(|k| (k, k)).try_collect().unwrap();

		assert_eq!(b1, b2.try_map::<_, (), _>(|(_, v)| Ok(v * 2)).unwrap());
	}

	#[test]
	fn try_map_short_circuit() {
		let b1: BoundedBTreeMap<u8, u8, ConstU32<7>> = [1, 2, 3, 4].into_iter().map(|k| (k, k)).try_collect().unwrap();

		assert_eq!(Err("overflow"), b1.try_map(|(_, v)| v.checked_mul(100).ok_or("overflow")));
	}

	#[test]
	fn try_map_ok() {
		let b1: BoundedBTreeMap<u8, u8, ConstU32<7>> = [1, 2, 3, 4].into_iter().map(|k| (k, k)).try_collect().unwrap();
		let b2: BoundedBTreeMap<u8, u16, ConstU32<7>> =
			[1, 2, 3, 4].into_iter().map(|k| (k, (k as u16) * 100)).try_collect().unwrap();

		assert_eq!(Ok(b2), b1.try_map(|(_, v)| (v as u16).checked_mul(100_u16).ok_or("overflow")));
	}

	// Just a test that structs containing `BoundedBTreeMap` can derive `Hash`. (This was broken
	// when it was deriving `Hash`).
	#[test]
	#[cfg(feature = "std")]
	fn container_can_derive_hash() {
		#[derive(Hash, Default)]
		struct Foo {
			bar: u8,
			map: BoundedBTreeMap<String, usize, ConstU32<16>>,
		}
		let _foo = Foo::default();
	}

	#[cfg(feature = "serde")]
	mod serde {
		use super::*;
		use crate::alloc::string::ToString;

		#[test]
		fn test_bounded_btreemap_serializer() {
			let mut map = BoundedBTreeMap::<u32, u32, ConstU32<6>>::new();
			map.try_insert(0, 100).unwrap();
			map.try_insert(1, 101).unwrap();
			map.try_insert(2, 102).unwrap();

			let serialized = serde_json::to_string(&map).unwrap();
			assert_eq!(serialized, r#"{"0":100,"1":101,"2":102}"#);
		}

		#[test]
		fn test_bounded_btreemap_deserializer() {
			let json_str = r#"{"0":100,"1":101,"2":102}"#;
			let map: Result<BoundedBTreeMap<u32, u32, ConstU32<6>>, serde_json::Error> = serde_json::from_str(json_str);
			assert!(map.is_ok());
			let map = map.unwrap();

			assert_eq!(map.len(), 3);
			assert_eq!(map.get(&0), Some(&100));
			assert_eq!(map.get(&1), Some(&101));
			assert_eq!(map.get(&2), Some(&102));
		}

		#[test]
		fn test_bounded_btreemap_deserializer_bound() {
			let json_str = r#"{"0":100,"1":101,"2":102}"#;
			let map: Result<BoundedBTreeMap<u32, u32, ConstU32<3>>, serde_json::Error> = serde_json::from_str(json_str);
			assert!(map.is_ok());
			let map = map.unwrap();

			assert_eq!(map.len(), 3);
			assert_eq!(map.get(&0), Some(&100));
			assert_eq!(map.get(&1), Some(&101));
			assert_eq!(map.get(&2), Some(&102));
		}

		#[test]
		fn test_bounded_btreemap_deserializer_failed() {
			let json_str = r#"{"0":100,"1":101,"2":102,"3":103,"4":104}"#;
			let map: Result<BoundedBTreeMap<u32, u32, ConstU32<4>>, serde_json::Error> = serde_json::from_str(json_str);

			match map {
				Err(e) => {
					assert!(e.to_string().contains("map exceeds the size of the bounds"));
				},
				_ => unreachable!("deserializer must raise error"),
			}
		}
	}

	#[test]
	fn is_full_works() {
		let mut bounded = boundedmap_from_keys::<u32, ConstU32<4>>(&[1, 2, 3]);
		assert!(!bounded.is_full());
		bounded.try_insert(0, ()).unwrap();
		assert_eq!(*bounded, map_from_keys(&[1, 0, 2, 3]));

		assert!(bounded.is_full());
		assert!(bounded.try_insert(9, ()).is_err());
		assert_eq!(*bounded, map_from_keys(&[1, 0, 2, 3]));
	}
}
