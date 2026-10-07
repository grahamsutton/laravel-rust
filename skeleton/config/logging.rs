use laravel::prelude::*;

pub fn config() -> Value {
    json!({
        /*
        |--------------------------------------------------------------------------
        | Default Log Channel
        |--------------------------------------------------------------------------
        |
        | This option defines the default log channel that is utilized to write
        | messages to your logs. The value provided here should match one of
        | the channels present in the list of "channels" configured below.
        |
        */

        "default": env("LOG_CHANNEL", "stack"),

        /*
        |--------------------------------------------------------------------------
        | Deprecations Log Channel
        |--------------------------------------------------------------------------
        |
        | This option controls the log channel that should be used to log warnings
        | regarding deprecated features. This allows you to get your application
        | ready for upcoming major versions of dependencies.
        |
        */

        "deprecations": {
            "channel": env("LOG_DEPRECATIONS_CHANNEL", "null"),
            "trace": env("LOG_DEPRECATIONS_TRACE", false).truthy(),
        },

        /*
        |--------------------------------------------------------------------------
        | Log Channels
        |--------------------------------------------------------------------------
        |
        | Here you may configure the log channels for your application. Laravel
        | ships with a variety of channel drivers, giving you a variety of
        | powerful log handlers / formatters to utilize.
        |
        | Available drivers: "single", "daily", "stderr", "errorlog", "custom", "stack"
        |
        */

        "channels": {
            "stack": {
                "driver": "stack",
                "channels": env("LOG_STACK", "single")
                    .to_string_lossy()
                    .split(',')
                    .map(str::trim)
                    .filter(|channel| !channel.is_empty())
                    .collect::<Vec<_>>(),
                "ignore_exceptions": false,
            },

            "single": {
                "driver": "single",
                "path": storage_path("logs/laravel.log"),
                "level": env("LOG_LEVEL", "debug"),
                "replace_placeholders": true,
            },

            "daily": {
                "driver": "daily",
                "path": storage_path("logs/laravel.log"),
                "level": env("LOG_LEVEL", "debug"),
                "days": env("LOG_DAILY_DAYS", 14),
                "replace_placeholders": true,
            },

            "stderr": {
                "driver": "stderr",
                "level": env("LOG_LEVEL", "debug"),
            },

            "errorlog": {
                "driver": "errorlog",
                "level": env("LOG_LEVEL", "debug"),
                "replace_placeholders": true,
            },

            "null": {
                "driver": "null",
            },

            "emergency": {
                "path": storage_path("logs/laravel.log"),
            },
        },
    })
}
