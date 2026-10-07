//! Command arguments.

use redis::ToRedisArgs;

use illuminate_support::Value;

/// Anything that can be passed as the arguments of a Redis command.
///
/// Tuples mix types freely, arrays and vectors pass many values of one
/// type, `()` passes nothing, and a JSON [`Value`] array works just like the
/// PHP array you would hand Laravel's `Redis::command`:
///
/// ```
/// use illuminate_redis::CommandArgs;
/// use illuminate_support::json;
///
/// assert_eq!(("names", 5, 10).into_args(), [b"names".to_vec(), b"5".to_vec(), b"10".to_vec()]);
/// assert_eq!(["first", "second"].into_args().len(), 2);
/// assert_eq!(().into_args().len(), 0);
/// assert_eq!(json!(["names", 5, 10]).into_args(), ("names", 5, 10).into_args());
/// ```
pub trait CommandArgs {
    /// Flatten the arguments into Redis' wire format.
    fn into_args(self) -> Vec<Vec<u8>>;
}

impl CommandArgs for () {
    fn into_args(self) -> Vec<Vec<u8>> {
        Vec::new()
    }
}

macro_rules! scalar_args {
    ($($type:ty),* $(,)?) => {
        $(
            impl CommandArgs for $type {
                fn into_args(self) -> Vec<Vec<u8>> {
                    self.to_redis_args()
                }
            }
        )*
    };
}

scalar_args!(
    &str, String, i8, i16, i32, i64, i128, isize, u16, u32, u64, u128, usize, f32, f64, bool,
);

impl CommandArgs for &String {
    fn into_args(self) -> Vec<Vec<u8>> {
        self.as_str().to_redis_args()
    }
}

impl<T: ToRedisArgs> CommandArgs for Vec<T> {
    fn into_args(self) -> Vec<Vec<u8>> {
        self.to_redis_args()
    }
}

impl<T: ToRedisArgs> CommandArgs for &Vec<T> {
    fn into_args(self) -> Vec<Vec<u8>> {
        self.as_slice().to_redis_args()
    }
}

impl<T: ToRedisArgs> CommandArgs for &[T] {
    fn into_args(self) -> Vec<Vec<u8>> {
        self.to_redis_args()
    }
}

impl<T: ToRedisArgs, const N: usize> CommandArgs for [T; N] {
    fn into_args(self) -> Vec<Vec<u8>> {
        (&self).to_redis_args()
    }
}

impl<T: ToRedisArgs, const N: usize> CommandArgs for &[T; N] {
    fn into_args(self) -> Vec<Vec<u8>> {
        self.to_redis_args()
    }
}

macro_rules! tuple_args {
    ($($name:ident),+) => {
        impl<$($name: ToRedisArgs),+> CommandArgs for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_args(self) -> Vec<Vec<u8>> {
                let ($($name,)+) = self;
                let mut args = Vec::new();
                $(args.extend($name.to_redis_args());)+
                args
            }
        }
    };
}

tuple_args!(A);
tuple_args!(A, B);
tuple_args!(A, B, C);
tuple_args!(A, B, C, D);
tuple_args!(A, B, C, D, E);
tuple_args!(A, B, C, D, E, F);
tuple_args!(A, B, C, D, E, F, G);
tuple_args!(A, B, C, D, E, F, G, H);
tuple_args!(A, B, C, D, E, F, G, H, I);
tuple_args!(A, B, C, D, E, F, G, H, I, J);
tuple_args!(A, B, C, D, E, F, G, H, I, J, K);
tuple_args!(A, B, C, D, E, F, G, H, I, J, K, L);

/// JSON values flatten like PHP arrays do: lists pass each item, objects
/// pass `key value` pairs (handy for `MSET` and `HSET`), and `null` passes
/// nothing.
impl CommandArgs for Value {
    fn into_args(self) -> Vec<Vec<u8>> {
        let mut args = Vec::new();
        flatten(self, &mut args);
        args
    }
}

impl CommandArgs for &Value {
    fn into_args(self) -> Vec<Vec<u8>> {
        self.clone().into_args()
    }
}

fn flatten(value: Value, args: &mut Vec<Vec<u8>>) {
    match value {
        Value::Null => {}
        Value::Bool(value) => args.push(if value { b"1".to_vec() } else { b"0".to_vec() }),
        Value::Number(number) => args.push(number.to_string().into_bytes()),
        Value::String(string) => args.push(string.into_bytes()),
        Value::Array(items) => items.into_iter().for_each(|item| flatten(item, args)),
        Value::Object(map) => {
            for (key, value) in map {
                args.push(key.into_bytes());
                flatten(value, args);
            }
        }
    }
}

/// Concatenate argument lists.
pub(crate) fn concat(parts: impl IntoIterator<Item = Vec<Vec<u8>>>) -> Vec<Vec<u8>> {
    parts.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_support::json;

    fn strings(args: Vec<Vec<u8>>) -> Vec<String> {
        args.into_iter()
            .map(|arg| String::from_utf8(arg).unwrap())
            .collect()
    }

    #[test]
    fn scalars_tuples_and_collections_flatten() {
        assert_eq!(strings("key".into_args()), ["key"]);
        assert_eq!(strings(String::from("key").into_args()), ["key"]);
        assert_eq!(strings((&String::from("key")).into_args()), ["key"]);
        assert_eq!(strings(42_i64.into_args()), ["42"]);
        assert_eq!(strings(1.5_f64.into_args()), ["1.5"]);
        assert_eq!(strings(true.into_args()), ["1"]);
        assert_eq!(
            strings(("a", 1, 2.5, false).into_args()),
            ["a", "1", "2.5", "0"]
        );
        assert_eq!(strings(vec!["a", "b"].into_args()), ["a", "b"]);
        assert_eq!(strings((&vec![1, 2]).into_args()), ["1", "2"]);
        assert_eq!(strings(["a", "b", "c"].into_args()), ["a", "b", "c"]);
        assert_eq!(strings((&["a"]).into_args()), ["a"]);
        assert_eq!(strings((&["x", "y"][..]).into_args()), ["x", "y"]);
        assert_eq!(
            strings(("key", vec!["a", "b"]).into_args()),
            ["key", "a", "b"]
        );
        assert!(().into_args().is_empty());
    }

    #[test]
    fn bytes_are_a_single_argument() {
        assert_eq!(
            vec![0_u8, 159, 146, 150].into_args(),
            [vec![0, 159, 146, 150]]
        );
        assert_eq!((&[1_u8, 2][..]).into_args(), [vec![1, 2]]);
    }

    #[test]
    fn json_values_flatten_like_php_arrays() {
        assert_eq!(
            strings(json!(["names", 5, 1.5, true, null, ["nested"]]).into_args()),
            ["names", "5", "1.5", "1", "nested"]
        );
        assert_eq!(
            strings(json!({"first": "Taylor", "last": "Otwell"}).into_args()),
            ["first", "Taylor", "last", "Otwell"]
        );
        assert_eq!(strings((&json!("x")).into_args()), ["x"]);
        assert_eq!(
            strings(concat([("a",).into_args(), ["b", "c"].into_args()])),
            ["a", "b", "c"]
        );
    }
}
