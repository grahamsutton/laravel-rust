//! Key prefixing.
//!
//! Like phpredis' `OPT_PREFIX`, a connection's prefix is applied to the
//! *keys* of a command — not to its values — so every command needs to know
//! which of its arguments are keys. This table covers Redis' key-taking
//! commands; arguments of any other command are passed through untouched.

/// Prepend the prefix to every key argument of the command.
pub(crate) fn prefix_keys(command: &str, args: &mut [Vec<u8>], prefix: &str) {
    if prefix.is_empty() {
        return;
    }
    for index in key_positions(command, args) {
        let key = &mut args[index];
        let mut prefixed = Vec::with_capacity(prefix.len() + key.len());
        prefixed.extend_from_slice(prefix.as_bytes());
        prefixed.extend_from_slice(key);
        *key = prefixed;
    }
}

/// The positions of the command's key arguments.
pub(crate) fn key_positions(command: &str, args: &[Vec<u8>]) -> Vec<usize> {
    let count = args.len();
    let all = |from: usize, to: usize| (from..to.min(count)).collect::<Vec<_>>();

    match command.to_ascii_lowercase().as_str() {
        // Every argument is a key...
        "del" | "unlink" | "exists" | "touch" | "mget" | "watch" | "sinter" | "sunion"
        | "sdiff" | "sinterstore" | "sunionstore" | "sdiffstore" | "pfcount" | "pfmerge" => {
            all(0, count)
        }

        // Every argument but the trailing timeout...
        "blpop" | "brpop" | "bzpopmin" | "bzpopmax" => all(0, count.saturating_sub(1)),

        // Keys and values alternate...
        "mset" | "msetnx" => (0..count).step_by(2).collect(),

        // A source and a destination...
        "rename" | "renamenx" | "rpoplpush" | "brpoplpush" | "lmove" | "blmove" | "smove"
        | "copy" | "zrangestore" | "geosearchstore" => all(0, 2),

        // An operation, a destination, then the source keys...
        "bitop" => all(1, count),

        // A key count, then the keys...
        "eval" | "evalsha" | "eval_ro" | "evalsha_ro" | "fcall" | "fcall_ro" | "blmpop"
        | "bzmpop" => counted_keys(args, 1),
        "zunion" | "zinter" | "zdiff" | "zintercard" | "sintercard" | "lmpop" | "zmpop" => {
            counted_keys(args, 0)
        }

        // A destination, a key count, then the keys...
        "zunionstore" | "zinterstore" | "zdiffstore" => {
            let mut keys = all(0, 1);
            keys.extend(counted_keys(args, 1));
            keys
        }

        command if FIRST_ARGUMENT_IS_A_KEY.contains(&command) => all(0, 1),

        _ => Vec::new(),
    }
}

/// The keys following the key count at `index` (`EVAL script 2 a b ...`).
fn counted_keys(args: &[Vec<u8>], index: usize) -> Vec<usize> {
    let keys = args
        .get(index)
        .and_then(|count| std::str::from_utf8(count).ok())
        .and_then(|count| count.trim().parse::<usize>().ok())
        .unwrap_or(0);
    (index + 1..(index + 1 + keys).min(args.len())).collect()
}

/// Commands whose first argument is their only key.
#[rustfmt::skip]
const FIRST_ARGUMENT_IS_A_KEY: &[&str] = &[
    // Strings...
    "get", "set", "setex", "psetex", "setnx", "getset", "getdel", "getex", "append", "strlen",
    "incr", "incrby", "incrbyfloat", "decr", "decrby", "getrange", "setrange", "substr",
    "getbit", "setbit", "bitcount", "bitpos", "bitfield", "bitfield_ro", "lcs",
    // Keys...
    "expire", "pexpire", "expireat", "pexpireat", "expiretime", "pexpiretime", "persist", "ttl",
    "pttl", "type", "dump", "restore", "sort", "sort_ro", "keys",
    // Hashes...
    "hget", "hset", "hsetnx", "hmset", "hmget", "hgetall", "hdel", "hexists", "hincrby",
    "hincrbyfloat", "hkeys", "hvals", "hlen", "hstrlen", "hscan", "hrandfield", "hexpire",
    "hpexpire", "httl", "hpttl", "hpersist",
    // Lists...
    "lpush", "rpush", "lpushx", "rpushx", "lpop", "rpop", "lrange", "llen", "lindex", "lset",
    "lrem", "ltrim", "linsert", "lpos",
    // Sets...
    "sadd", "srem", "smembers", "sismember", "smismember", "scard", "spop", "srandmember",
    "sscan",
    // Sorted sets...
    "zadd", "zincrby", "zrange", "zrevrange", "zrangebyscore", "zrevrangebyscore",
    "zrangebylex", "zrevrangebylex", "zrem", "zremrangebyscore", "zremrangebyrank",
    "zremrangebylex", "zcard", "zcount", "zlexcount", "zscore", "zmscore", "zrank", "zrevrank",
    "zscan", "zpopmin", "zpopmax", "zrandmember",
    // HyperLogLogs, geospatial indexes and streams...
    "pfadd", "geoadd", "geopos", "geodist", "geohash", "georadius", "georadius_ro",
    "georadiusbymember", "georadiusbymember_ro", "geosearch", "xadd", "xlen", "xrange",
    "xrevrange", "xdel", "xtrim", "xack", "xclaim", "xautoclaim", "xpending", "xsetid",
    // Pub/sub...
    "publish", "spublish",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::CommandArgs;

    fn prefixed(command: &str, args: impl CommandArgs) -> Vec<String> {
        let mut args = args.into_args();
        prefix_keys(command, &mut args, "app:");
        args.into_iter()
            .map(|arg| String::from_utf8(arg).unwrap())
            .collect()
    }

    #[test]
    fn single_key_commands_prefix_their_first_argument() {
        assert_eq!(prefixed("get", ("name",)), ["app:name"]);
        assert_eq!(prefixed("SET", ("name", "Taylor")), ["app:name", "Taylor"]);
        assert_eq!(
            prefixed("lrange", ("names", 0, -1)),
            ["app:names", "0", "-1"]
        );
        assert_eq!(prefixed("zadd", ("board", 1, "a")), ["app:board", "1", "a"]);
        assert_eq!(prefixed("keys", ("queues:*",)), ["app:queues:*"]);
        assert_eq!(prefixed("publish", ("chat", "hi")), ["app:chat", "hi"]);
        assert_eq!(prefixed("get", ()), Vec::<String>::new());
    }

    #[test]
    fn multi_key_commands_prefix_every_key() {
        assert_eq!(prefixed("del", ["a", "b"]), ["app:a", "app:b"]);
        assert_eq!(prefixed("mget", ["a", "b"]), ["app:a", "app:b"]);
        assert_eq!(
            prefixed("mset", ("a", 1, "b", 2)),
            ["app:a", "1", "app:b", "2"]
        );
        assert_eq!(prefixed("blpop", ("a", "b", 5)), ["app:a", "app:b", "5"]);
        assert_eq!(prefixed("rename", ("a", "b")), ["app:a", "app:b"]);
        assert_eq!(prefixed("smove", ("a", "b", "m")), ["app:a", "app:b", "m"]);
        assert_eq!(
            prefixed("bitop", ("AND", "dest", "a", "b")),
            ["AND", "app:dest", "app:a", "app:b"]
        );
    }

    #[test]
    fn counted_keys_are_prefixed() {
        assert_eq!(
            prefixed("eval", ("return 1", 2, "a", "b", "arg")),
            ["return 1", "2", "app:a", "app:b", "arg"]
        );
        assert_eq!(prefixed("evalsha", ("abc", 0, "arg")), ["abc", "0", "arg"]);
        assert_eq!(
            prefixed("zunionstore", ("dest", 2, "a", "b", "WEIGHTS", 1, 2)),
            ["app:dest", "2", "app:a", "app:b", "WEIGHTS", "1", "2"]
        );
        assert_eq!(prefixed("zunion", (2, "a", "b")), ["2", "app:a", "app:b"]);
        // A key count larger than the arguments is clamped...
        assert_eq!(prefixed("eval", ("s", 5, "a")), ["s", "5", "app:a"]);
    }

    #[test]
    fn other_commands_are_untouched() {
        assert_eq!(prefixed("ping", ()), Vec::<String>::new());
        assert_eq!(prefixed("echo", ("hi",)), ["hi"]);
        assert_eq!(prefixed("select", (1,)), ["1"]);

        let mut args = ("name",).into_args();
        prefix_keys("get", &mut args, "");
        assert_eq!(args, [b"name".to_vec()]);
    }
}
