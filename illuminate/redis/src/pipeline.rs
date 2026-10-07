//! Pipelines and transactions.

use redis::ToRedisArgs;

use crate::args::{CommandArgs, concat};
use crate::keys::prefix_keys;

/// Commands queued by [`Connection::pipeline`](crate::Connection::pipeline)
/// and [`Connection::transaction`](crate::Connection::transaction).
///
/// Every method queues a command — nothing is sent until the closure
/// returns, when all of the commands go to the server in a single round
/// trip. Their replies are returned in order.
///
/// ```no_run
/// use illuminate_redis::Redis;
///
/// # async fn example() -> illuminate_support::Result<()> {
/// let replies = Redis::pipeline(|pipe| {
///     for i in 0..1000 {
///         pipe.set(format!("key:{i}"), i);
///     }
/// }).await?;
///
/// assert_eq!(replies.len(), 1000);
/// # Ok(())
/// # }
/// ```
pub struct Pipeline {
    prefix: String,
    commands: Vec<redis::Cmd>,
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("prefix", &self.prefix)
            .field("commands", &self.commands.len())
            .finish()
    }
}

impl Pipeline {
    pub(crate) fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            commands: Vec::new(),
        }
    }

    /// The queued commands, ready to be sent.
    pub(crate) fn into_pipe(self, atomic: bool) -> redis::Pipeline {
        let mut pipe = redis::pipe();
        if atomic {
            pipe.atomic();
        }
        for command in self.commands {
            pipe.add_command(command);
        }
        pipe
    }

    /// The number of queued commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Determine if no commands have been queued.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Queue any Redis command.
    pub fn command(&mut self, method: &str, args: impl CommandArgs) -> &mut Self {
        let mut args = args.into_args();
        prefix_keys(method, &mut args, &self.prefix);
        self.commands.push(build_command(method, args));
        self
    }

    /// Queue a `GET`.
    pub fn get(&mut self, key: impl AsRef<str>) -> &mut Self {
        self.command("get", (key.as_ref(),))
    }

    /// Queue a `SET`.
    pub fn set(&mut self, key: impl AsRef<str>, value: impl ToRedisArgs) -> &mut Self {
        self.command("set", (key.as_ref(), value))
    }

    /// Queue a `SETEX`.
    pub fn set_ex(
        &mut self,
        key: impl AsRef<str>,
        value: impl ToRedisArgs,
        seconds: u64,
    ) -> &mut Self {
        self.command("setex", (key.as_ref(), seconds, value))
    }

    /// Queue a `SETNX`.
    pub fn setnx(&mut self, key: impl AsRef<str>, value: impl ToRedisArgs) -> &mut Self {
        self.command("setnx", (key.as_ref(), value))
    }

    /// Queue a `DEL` of one or more keys.
    pub fn del(&mut self, keys: impl CommandArgs) -> &mut Self {
        self.command("del", keys)
    }

    /// Queue an `INCR`.
    pub fn incr(&mut self, key: impl AsRef<str>) -> &mut Self {
        self.command("incr", (key.as_ref(),))
    }

    /// Queue an `INCRBY`.
    pub fn incrby(&mut self, key: impl AsRef<str>, amount: i64) -> &mut Self {
        self.command("incrby", (key.as_ref(), amount))
    }

    /// Queue a `DECR`.
    pub fn decr(&mut self, key: impl AsRef<str>) -> &mut Self {
        self.command("decr", (key.as_ref(),))
    }

    /// Queue a `DECRBY`.
    pub fn decrby(&mut self, key: impl AsRef<str>, amount: i64) -> &mut Self {
        self.command("decrby", (key.as_ref(), amount))
    }

    /// Queue an `EXPIRE`.
    pub fn expire(&mut self, key: impl AsRef<str>, seconds: i64) -> &mut Self {
        self.command("expire", (key.as_ref(), seconds))
    }

    /// Queue an `HSET`.
    pub fn hset(
        &mut self,
        key: impl AsRef<str>,
        field: impl AsRef<str>,
        value: impl ToRedisArgs,
    ) -> &mut Self {
        self.command("hset", (key.as_ref(), field.as_ref(), value))
    }

    /// Queue an `HDEL` of one or more fields.
    pub fn hdel(&mut self, key: impl AsRef<str>, fields: impl CommandArgs) -> &mut Self {
        self.command(
            "hdel",
            concat([(key.as_ref(),).into_args(), fields.into_args()]),
        )
    }

    /// Queue an `LPUSH` of one or more values.
    pub fn lpush(&mut self, key: impl AsRef<str>, values: impl CommandArgs) -> &mut Self {
        self.command(
            "lpush",
            concat([(key.as_ref(),).into_args(), values.into_args()]),
        )
    }

    /// Queue an `RPUSH` of one or more values.
    pub fn rpush(&mut self, key: impl AsRef<str>, values: impl CommandArgs) -> &mut Self {
        self.command(
            "rpush",
            concat([(key.as_ref(),).into_args(), values.into_args()]),
        )
    }

    /// Queue an `SADD` of one or more members.
    pub fn sadd(&mut self, key: impl AsRef<str>, members: impl CommandArgs) -> &mut Self {
        self.command(
            "sadd",
            concat([(key.as_ref(),).into_args(), members.into_args()]),
        )
    }

    /// Queue an `SREM` of one or more members.
    pub fn srem(&mut self, key: impl AsRef<str>, members: impl CommandArgs) -> &mut Self {
        self.command(
            "srem",
            concat([(key.as_ref(),).into_args(), members.into_args()]),
        )
    }

    /// Queue a `ZADD`.
    pub fn zadd(
        &mut self,
        key: impl AsRef<str>,
        score: f64,
        member: impl ToRedisArgs,
    ) -> &mut Self {
        self.command("zadd", (key.as_ref(), score, member))
    }

    /// Queue a `ZREM` of one or more members.
    pub fn zrem(&mut self, key: impl AsRef<str>, members: impl CommandArgs) -> &mut Self {
        self.command(
            "zrem",
            concat([(key.as_ref(),).into_args(), members.into_args()]),
        )
    }

    /// Queue a `PUBLISH`.
    pub fn publish(&mut self, channel: impl AsRef<str>, message: impl ToRedisArgs) -> &mut Self {
        self.command("publish", (channel.as_ref(), message))
    }
}

/// Build a command from its name and (already prefixed) arguments.
pub(crate) fn build_command(method: &str, args: Vec<Vec<u8>>) -> redis::Cmd {
    let mut command = redis::cmd(method);
    for arg in args {
        command.arg(arg);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(pipeline: &Pipeline) -> String {
        String::from_utf8_lossy(
            &pipeline
                .commands
                .iter()
                .flat_map(|c| c.get_packed_command())
                .collect::<Vec<_>>(),
        )
        .into_owned()
    }

    #[test]
    fn commands_are_queued_with_prefixed_keys() {
        let mut pipe = Pipeline::new("app:");
        assert!(pipe.is_empty());
        pipe.set("name", "Taylor")
            .get("name")
            .set_ex("token", "abc", 0)
            .setnx("lock", 1)
            .del(["a", "b"])
            .incr("visits")
            .incrby("visits", 5)
            .decr("visits")
            .decrby("visits", 2)
            .expire("name", 60)
            .hset("user", "name", "Taylor")
            .hdel("user", ["name", "email"])
            .lpush("list", ["a"])
            .rpush("list", "b")
            .sadd("set", ["a", "b"])
            .srem("set", "a")
            .zadd("board", 1.5, "taylor")
            .zrem("board", ["taylor"])
            .publish("chat", "hi")
            .command("ping", ());
        assert_eq!(pipe.len(), 20);

        let packed = packed(&pipe);
        assert!(packed.contains("app:name"));
        assert!(packed.contains("app:token"));
        assert!(packed.contains("app:a"));
        assert!(packed.contains("app:user"));
        assert!(packed.contains("app:board"));
        assert!(packed.contains("app:chat"));
        assert!(!packed.contains("app:Taylor"));
        assert!(format!("{pipe:?}").contains("commands: 20"));

        let pipe = pipe.into_pipe(true);
        let packed = String::from_utf8_lossy(&pipe.get_packed_pipeline()).into_owned();
        assert!(packed.contains("MULTI") && packed.contains("EXEC"));
    }
}
