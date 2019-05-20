use uint::*;
use impl_rlp::impl_uint_rlp;
use rlp::Encodable;

construct_uint! { pub struct U256(32); }
impl_uint_rlp!(U256, 32);

fn is_encodable<T: Encodable>(_t: T) {}

#[cfg(test)]
mod tests {
	use super::*;

    #[test]
    fn u256_is_encodable() {
		let a = U256::zero();
		is_encodable(a);
    }
}
