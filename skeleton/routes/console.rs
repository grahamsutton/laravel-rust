use laravel::prelude::*;

/// The application's closure based console commands.
pub fn console() {
    Artisan::command("inspire", |cmd| async move {
        cmd.comment(Inspiring::quote());

        Ok(())
    })
    .purpose("Display an inspiring quote");
}
