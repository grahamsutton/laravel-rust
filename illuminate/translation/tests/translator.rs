use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use illuminate_config::Repository;
use illuminate_container::{Container, ServiceProvider};
use illuminate_support::{Value, json};
use illuminate_translation::{
    __, __with, ArrayLoader, FileLoader, Lang, Loader, TranslationServiceProvider, Translator,
    locale_scope, trans, trans_choice, trans_choice_with, trans_with, with_locale,
    with_locale_async,
};
use tempfile::TempDir;

fn write(dir: &Path, path: &str, contents: Value) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, serde_json::to_string_pretty(&contents).unwrap()).unwrap();
}

/// A `lang` directory with a little of everything.
fn lang() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = &dir.path().join("lang");
    write(
        path,
        "en/messages.json",
        json!({
            "welcome": "Welcome, :name!",
            "goodbye": "Goodbye, :NAME. See you, :Name.",
            "nested": {"title": "Nested :name", "deeper": {"line": "Deep"}},
            "apples": "There is one apple|There are many apples",
            "oranges": "{0} There are none|{1} There is one|[2,*] There are :count",
            "list": ["first :name", "second"],
            "only_en": "Only in English",
            "empty": "",
        }),
    );
    write(
        path,
        "es/messages.json",
        json!({
            "welcome": "Bienvenido, :name!",
            "nested": {"title": "Anidado :name"},
        }),
    );
    write(
        path,
        "fr/messages.json",
        json!({"apples": "une pomme|des pommes"}),
    );
    write(
        path,
        "en.json",
        json!({
            "I love programming.": "I love programming!",
            "messages.only_en": "The JSON line wins",
            "Same": "Same",
            "Blank": "",
            "Hello :name": "Hi :name",
            "There is one apple|There are many apples": "One apple|Many apples",
        }),
    );
    write(
        path,
        "es.json",
        json!({"I love programming.": "Me encanta programar."}),
    );
    write(path, "en/auth.json", json!({"failed": "Nope."}));
    write(path, "en/admin/users.json", json!({"title": "Users"}));
    write(
        path,
        "vendor/courier/en/messages.json",
        json!({"welcome": "Overridden courier welcome"}),
    );
    write(
        dir.path(),
        "courier-lang/en/messages.json",
        json!({
            "welcome": "Courier welcome",
            "other": "Courier other",
        }),
    );
    dir
}

fn lang_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("lang")
}

fn translator(dir: &TempDir, locale: &str) -> Translator {
    let translator = Translator::new(lang_path(dir), locale);
    translator.set_fallback("en");
    translator
}

// ----------------------------------------------------------------------
// Retrieving lines
// ----------------------------------------------------------------------

#[test]
fn group_lines_are_retrieved_with_dot_notation() {
    let dir = lang();
    let t = translator(&dir, "en");

    assert_eq!(t.get("messages.welcome"), "Welcome, :name!");
    assert_eq!(
        t.get_with("messages.welcome", &json!({"name": "dayle"})),
        "Welcome, dayle!"
    );
    assert_eq!(t.get("messages.nested.title"), "Nested :name");
    assert_eq!(t.get("messages.nested.deeper.line"), "Deep");
    assert_eq!(t.get("admin/users.title"), "Users");
}

#[test]
fn missing_lines_return_the_key_with_replacements() {
    let dir = lang();
    let t = translator(&dir, "en");

    assert_eq!(t.get("messages.missing"), "messages.missing");
    assert_eq!(t.get("missing.group"), "missing.group");
    assert_eq!(
        t.get_with("Welcome back, :name", &json!({"name": "taylor"})),
        "Welcome back, taylor"
    );
    assert_eq!(t.get(""), "");
}

#[test]
fn placeholders_follow_capitalization() {
    let dir = lang();
    let t = translator(&dir, "en");
    assert_eq!(
        t.get_with("messages.goodbye", &json!({"name": "dayle"})),
        "Goodbye, DAYLE. See you, Dayle."
    );
    // From Laravel's TranslationTranslatorTest...
    assert_eq!(
        t.get_with("breeze :Foo :BAR", &json!({"foo": "bar", "bar": "baz"})),
        "breeze Bar BAZ"
    );
    assert_eq!(
        t.get_with(
            "breeze :foo :foobar",
            &json!({"foo": "bar", "foobar": "taylor"})
        ),
        "breeze bar taylor"
    );
    assert_eq!(
        t.get_with("Hello :foo!", &json!({"foo": "baz:bar", "bar": "abcdef"})),
        "Hello baz:bar!"
    );
    assert_eq!(
        t.get_with(
            "foo :i:c :u",
            &json!({"i": "one", "c": "two", "u": "three"})
        ),
        "foo onetwo three"
    );
}

#[test]
fn json_strings_are_checked_first() {
    let dir = lang();
    let en = translator(&dir, "en");
    let es = translator(&dir, "es");

    assert_eq!(en.get("I love programming."), "I love programming!");
    assert_eq!(es.get("I love programming."), "Me encanta programar.");
    assert_eq!(en.get("messages.only_en"), "The JSON line wins");
    assert_eq!(
        en.get_with("Hello :name", &json!({"name": "Taylor"})),
        "Hi Taylor"
    );
    // A JSON key missing from the locale is returned as-is (JSON strings
    // don't fall back)...
    assert_eq!(es.get("Hello :name"), "Hello :name");
}

#[test]
fn empty_json_lines_return_the_key() {
    let dir = lang();
    let t = translator(&dir, "en");
    assert_eq!(t.get("Blank"), "Blank");
    // ...but an empty group line is still a line.
    assert_eq!(t.get("messages.empty"), "");
}

#[test]
fn lines_fall_back_to_the_fallback_locale() {
    let dir = lang();
    let es = translator(&dir, "es");

    assert_eq!(
        es.get_with("messages.welcome", &json!({"name": "dayle"})),
        "Bienvenido, dayle!"
    );
    assert_eq!(es.get("messages.nested.title"), "Anidado :name");
    assert_eq!(es.get("messages.only_en"), "Only in English");
    assert_eq!(
        es.get_value("messages.only_en", &Value::Null, None, false),
        json!("messages.only_en")
    );
    assert_eq!(
        es.get_in("messages.welcome", &json!({"name": "x"}), "en"),
        "Welcome, x!"
    );
}

#[test]
fn whole_groups_and_arrays_are_returned_as_values() {
    let dir = lang();
    let t = translator(&dir, "en");

    let nested = t.get_value("messages.nested", &json!({"name": "Taylor"}), None, true);
    assert_eq!(
        nested,
        json!({"title": "Nested Taylor", "deeper": {"line": "Deep"}})
    );

    let list = t.get_value("messages.list", &json!({"name": "a"}), None, true);
    assert_eq!(list, json!(["first a", "second"]));

    let group = t.get_value("auth", &Value::Null, None, true);
    assert_eq!(group["failed"], "Nope.");
    assert_eq!(
        group["throttle"],
        "Too many login attempts. Please try again in :seconds seconds."
    );

    let validation = t.get_value("validation.between", &Value::Null, None, true);
    assert_eq!(
        validation["string"],
        "The :attribute field must be between :min and :max characters."
    );
}

#[test]
fn has_checks_for_lines() {
    let dir = lang();
    let es = translator(&dir, "es");

    assert!(es.has("messages.welcome"));
    assert!(es.has("messages.only_en"));
    assert!(!es.has_for_locale("messages.only_en", "es"));
    assert!(es.has_for_locale("messages.welcome", "es"));
    assert!(!es.has("messages.missing"));
    assert!(es.has("I love programming."));
    assert!(
        es.has_in("Same", Some("en"), false),
        "a JSON line equal to its key still exists"
    );
    assert!(!es.has("Same"));
}

#[test]
fn framework_lines_are_built_in_and_may_be_overridden() {
    let dir = lang();
    let t = translator(&dir, "en");

    assert_eq!(t.get("auth.failed"), "Nope.");
    assert_eq!(
        t.get("auth.password"),
        "The provided password is incorrect."
    );
    assert_eq!(
        t.get_with("auth.throttle", &json!({"seconds": 30})),
        "Too many login attempts. Please try again in 30 seconds."
    );
    assert_eq!(t.get("pagination.previous"), "&laquo; Previous");
    assert_eq!(
        t.get("passwords.sent"),
        "We have emailed your password reset link."
    );
    assert_eq!(
        t.get_with("validation.required", &json!({"attribute": "email"})),
        "The email field is required."
    );
    assert_eq!(
        t.get_with(
            "validation.min.numeric",
            &json!({"attribute": "age", "min": 18})
        ),
        "The age field must be at least 18."
    );
    // Other locales fall back to the framework's English lines.
    let es = translator(&dir, "es");
    assert_eq!(
        es.get("auth.password"),
        "The provided password is incorrect."
    );
}

#[test]
fn framework_lines_can_be_disabled() {
    let dir = lang();
    let t = Translator::with_loader(
        FileLoader::new(lang_path(&dir)).without_framework_lines(),
        "en",
    );
    assert_eq!(t.get("auth.password"), "auth.password");
    assert_eq!(t.get("auth.failed"), "Nope.");
}

#[test]
fn namespaced_lines_with_vendor_overrides() {
    let dir = lang();
    let t = translator(&dir, "en");
    t.add_namespace("courier", dir.path().join("courier-lang"));

    assert_eq!(
        t.get("courier::messages.welcome"),
        "Overridden courier welcome"
    );
    assert_eq!(t.get("courier::messages.other"), "Courier other");
    assert_eq!(
        t.get("courier::messages.missing"),
        "courier::messages.missing"
    );
    assert_eq!(
        t.get("unknown::messages.welcome"),
        "unknown::messages.welcome"
    );
    assert!(t.namespaces().contains_key("courier"));
}

#[test]
fn lines_can_be_added_at_runtime() {
    let dir = lang();
    let t = translator(&dir, "en");

    t.add_lines(
        &json!({
            "messages.added": "Added :name",
            "messages.nested.extra": "Extra",
            "*.Runtime string.": "Runtime!",
        }),
        "en",
    );
    t.add_namespaced_lines(&json!({"messages.hello": "Package hello"}), "en", "package");

    assert_eq!(
        t.get_with("messages.added", &json!({"name": "x"})),
        "Added x"
    );
    assert_eq!(t.get("messages.nested.extra"), "Extra");
    // File lines in the same group are still there.
    assert_eq!(t.get("messages.nested.title"), "Nested :name");
    assert_eq!(t.get("messages.welcome"), "Welcome, :name!");
    assert_eq!(t.get("Runtime string."), "Runtime!");
    assert_eq!(t.get("package::messages.hello"), "Package hello");
}

#[test]
fn unsafe_locales_and_groups_are_ignored() {
    let dir = lang();
    let t = translator(&dir, "en");
    assert_eq!(
        t.get_value("messages.welcome", &Value::Null, Some("../en"), false),
        json!("messages.welcome")
    );
    assert_eq!(t.get("../en/messages.welcome"), "../en/messages.welcome");
    assert!(t.set_locale("../../etc").is_err());
    assert!(t.set_locale("en\\x").is_err());
    assert_eq!(
        t.set_locale("a/b").unwrap_err().to_string(),
        "Invalid characters present in locale."
    );
}

#[test]
fn invalid_json_files_are_reported_but_not_fatal() {
    let dir = lang();
    fs::write(lang_path(&dir).join("de.json"), "{not json").unwrap();
    fs::create_dir_all(lang_path(&dir).join("de")).unwrap();
    fs::write(lang_path(&dir).join("de/messages.json"), "[1,").unwrap();

    let loader = FileLoader::new(lang_path(&dir));
    assert!(loader.load("de", "*", Some("*")).is_err());
    assert!(loader.load("de", "messages", None).is_err());

    let t = translator(&dir, "de");
    assert_eq!(t.get("Hello"), "Hello");
    assert_eq!(t.get("messages.welcome"), "Welcome, :name!");
}

// ----------------------------------------------------------------------
// Pluralization
// ----------------------------------------------------------------------

#[test]
fn choice_selects_lines_by_count() {
    let dir = lang();
    let t = translator(&dir, "en");

    // The JSON line for this exact key is used...
    assert_eq!(
        t.choice("There is one apple|There are many apples", 1),
        "One apple"
    );
    assert_eq!(t.choice("messages.apples", 1), "There is one apple");
    assert_eq!(t.choice("messages.apples", 2), "There are many apples");
    assert_eq!(t.choice("messages.apples", 0), "There are many apples");
    assert_eq!(t.choice("messages.oranges", 0), "There are none");
    assert_eq!(t.choice("messages.oranges", 1), "There is one");
    assert_eq!(t.choice("messages.oranges", 7), "There are 7");
    assert_eq!(t.choice("messages.oranges", 2.5), "There are 2.5");
    assert_eq!(
        t.choice("messages.oranges", &vec!["a", "b", "c"]),
        "There are 3"
    );
    assert_eq!(t.choice("messages.oranges", &["a"][..]), "There is one");
    assert_eq!(
        t.choice("messages.oranges", &illuminate_support::collect(vec![1, 2])),
        "There are 2"
    );
}

#[test]
fn choice_replacements_include_count_unless_given() {
    let dir = lang();
    let t = translator(&dir, "en");
    t.add_lines(
        &json!({"time.minutes_ago": "{1} :value minute ago|[2,*] :value minutes ago"}),
        "en",
    );

    assert_eq!(
        t.choice_with("time.minutes_ago", 5, &json!({"value": 5})),
        "5 minutes ago"
    );
    assert_eq!(
        t.choice_with("time.minutes_ago", 1, &json!({"value": "one"})),
        "one minute ago"
    );
    assert_eq!(
        t.choice_with("messages.oranges", 3, &json!({"count": "three"})),
        "There are three"
    );
}

#[test]
fn choice_uses_the_rules_of_the_locale() {
    let dir = lang();
    let fr = translator(&dir, "fr");

    // French treats zero as singular.
    assert_eq!(fr.choice("messages.apples", 0), "une pomme");
    assert_eq!(fr.choice("messages.apples", 1), "une pomme");
    assert_eq!(fr.choice("messages.apples", 2), "des pommes");
    assert_eq!(
        fr.choice_in("messages.apples", 0, &Value::Null, "en"),
        "There are many apples"
    );
}

#[test]
fn choice_falls_back_to_the_fallback_locale_and_its_rules() {
    let dir = lang();
    let es = translator(&dir, "es");
    assert_eq!(es.choice("messages.oranges", 4), "There are 4");
    assert_eq!(es.choice("messages.apples", 1), "There is one apple");
}

#[test]
fn choice_handles_many_plural_forms() {
    let loader = ArrayLoader::new();
    loader.add_messages(
        "ru",
        "files",
        json!({"count": ":count файл|:count файла|:count файлов"}),
        None,
    );
    loader.add_messages(
        "ar",
        "files",
        json!({"count": "صفر|واحد|اثنان|قليل|كثير|أخرى"}),
        None,
    );
    loader.add_messages("ja", "files", json!({"count": ":count ファイル"}), None);
    loader.add_messages(
        "cs",
        "files",
        json!({"count": "soubor|soubory|souborů"}),
        None,
    );
    let t = Translator::with_loader(loader, "ru");
    t.set_fallback("en");

    assert_eq!(t.choice("files.count", 1), "1 файл");
    assert_eq!(t.choice("files.count", 21), "21 файл");
    assert_eq!(t.choice("files.count", 3), "3 файла");
    assert_eq!(t.choice("files.count", 11), "11 файлов");
    assert_eq!(t.choice("files.count", 25), "25 файлов");

    let ar = |n: i64| t.choice_in("files.count", n, &Value::Null, "ar");
    assert_eq!(
        [ar(0), ar(1), ar(2), ar(5), ar(50), ar(100)],
        ["صفر", "واحد", "اثنان", "قليل", "كثير", "أخرى"]
    );

    assert_eq!(
        t.choice_in("files.count", 100, &Value::Null, "ja"),
        "100 ファイル"
    );
    assert_eq!(t.choice_in("files.count", 4, &Value::Null, "cs"), "soubory");
    assert_eq!(t.choice_in("files.count", 5, &Value::Null, "cs"), "souborů");
}

// ----------------------------------------------------------------------
// Missing keys & locales
// ----------------------------------------------------------------------

#[test]
fn missing_keys_can_be_handled() {
    let dir = lang();
    let t = Arc::new(translator(&dir, "en"));
    type Calls = Arc<Mutex<Vec<(String, Value, String, bool)>>>;
    let missing: Calls = Arc::default();

    let seen = missing.clone();
    let inner = Arc::downgrade(&t);
    t.handle_missing_keys_using(move |key, replace, locale, fallback| {
        seen.lock().unwrap().push((
            key.to_string(),
            replace.clone(),
            locale.to_string(),
            fallback,
        ));
        // Translating inside the handler must not loop forever...
        let nested = inner.upgrade().unwrap().get("another.missing.key");
        Some(format!("{key} ({nested})"))
    });

    assert_eq!(
        t.get_with("messages.nope", &json!({"a": 1})),
        "messages.nope (another.missing.key)"
    );
    assert_eq!(t.get("messages.welcome"), "Welcome, :name!");
    assert!(!t.has("messages.also_missing"));

    let missing = missing.lock().unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(
        missing[0],
        (
            "messages.nope".to_string(),
            json!({"a": 1}),
            "en".to_string(),
            true
        )
    );
    drop(missing);

    t.forget_missing_keys_handler();
    assert_eq!(t.get("messages.nope"), "messages.nope");
}

#[test]
fn locales_can_be_determined_by_a_callback() {
    let loader = ArrayLoader::new();
    loader.add_messages("de", "messages", json!({"hello": "Hallo"}), None);
    loader.add_messages(
        "en",
        "messages",
        json!({"hello": "Hello", "bye": "Bye"}),
        None,
    );
    let t = Translator::with_loader(loader, "de_CH");
    t.set_fallback("en");

    assert_eq!(t.get("messages.hello"), "Hello");

    t.determine_locales_using(|locales| {
        let mut expanded = Vec::new();
        for locale in locales {
            if let Some((language, _)) = locale.split_once('_') {
                expanded.push(locale.clone());
                expanded.push(language.to_string());
            } else {
                expanded.push(locale);
            }
        }
        expanded
    });

    assert_eq!(t.get("messages.hello"), "Hallo");
    assert_eq!(t.get("messages.bye"), "Bye");
}

#[test]
fn locales_can_be_set_and_checked() {
    let dir = lang();
    let t = translator(&dir, "en");

    assert_eq!(t.locale(), "en");
    assert!(t.is_locale("en"));
    t.set_locale("es").unwrap();
    assert_eq!(t.get_locale(), "es");
    assert_eq!(t.get("I love programming."), "Me encanta programar.");
    assert_eq!(t.get_fallback().as_deref(), Some("en"));

    assert_eq!(
        with_locale("en", || t.get("I love programming.")),
        "I love programming!"
    );
    assert_eq!(t.get_locale(), "es");
}

#[tokio::test]
async fn locale_scopes_keep_requests_apart() {
    let dir = lang();
    let t = Arc::new(translator(&dir, "en"));

    let request = |locale: &'static str| {
        let t = t.clone();
        tokio::spawn(locale_scope(async move {
            t.set_locale(locale).unwrap();
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            (t.get_locale(), t.get("I love programming."))
        }))
    };

    let (es, en) = tokio::join!(request("es"), request("en"));
    assert_eq!(
        es.unwrap(),
        ("es".to_string(), "Me encanta programar.".to_string())
    );
    assert_eq!(
        en.unwrap(),
        ("en".to_string(), "I love programming!".to_string())
    );

    // The default locale was never touched.
    assert_eq!(t.get_locale(), "en");

    let inside = with_locale_async("es", async { t.get("I love programming.") }).await;
    assert_eq!(inside, "Me encanta programar.");

    // Nested scopes inherit the outer request's locale.
    locale_scope(async {
        t.set_locale("es").unwrap();
        let nested = locale_scope(async { t.get_locale() }).await;
        assert_eq!(nested, "es");
    })
    .await;
    assert_eq!(t.get_locale(), "en");
}

// ----------------------------------------------------------------------
// Facade, helpers & provider
// ----------------------------------------------------------------------

fn app(dir: &TempDir, locale: &str) -> (Arc<Container>, illuminate_container::LocalInstanceGuard) {
    let app = Arc::new(Container::new());
    let guard = Container::set_local_instance(app.clone());
    app.instance(Repository::new(json!({
        "app": {"locale": locale, "fallback_locale": "en", "lang_path": lang_path(dir)},
    })));
    TranslationServiceProvider.register(&app);
    (app, guard)
}

#[test]
fn helpers_translate_through_the_container() {
    let dir = lang();
    let (_app, _guard) = app(&dir, "es");

    assert_eq!(__("I love programming."), "Me encanta programar.");
    assert_eq!(trans("messages.only_en"), "Only in English");
    assert_eq!(
        __with("messages.welcome", json!({"name": "dayle"})),
        "Bienvenido, dayle!"
    );
    assert_eq!(
        trans_with("messages.welcome", json!({"name": "taylor"})),
        "Bienvenido, taylor!"
    );
    assert_eq!(trans_choice("messages.oranges", 0), "There are none");
    assert_eq!(
        trans_choice_with("messages.oranges", 9, json!({})),
        "There are 9"
    );

    Lang::set_locale("en").unwrap();
    assert!(Lang::is_locale("en"));
    assert_eq!(Lang::get("I love programming."), "I love programming!");
    assert_eq!(
        Lang::get_in("I love programming.", &Value::Null, "es"),
        "Me encanta programar."
    );
    assert!(Lang::has("messages.welcome"));
    assert!(Lang::has_for_locale("messages.welcome", "es"));
    assert_eq!(
        Lang::choice_in("messages.apples", 0, &Value::Null, "fr"),
        "une pomme"
    );
    assert_eq!(Lang::with_locale("es", Lang::locale), "es");
}

#[test]
fn the_provider_binds_a_swappable_loader() {
    let dir = lang();
    let app = Arc::new(Container::new());
    let _guard = Container::set_local_instance(app.clone());
    app.instance(Repository::new(
        json!({"app": {"locale": "en", "lang_path": lang_path(&dir)}}),
    ));
    TranslationServiceProvider.register(&app);

    let loader = ArrayLoader::new();
    loader.add_messages("en", "messages", json!({"welcome": "From memory"}), None);
    let loader: Arc<dyn Loader> = Arc::new(loader);
    app.instance_arc::<dyn Loader>(loader);

    assert_eq!(Lang::get("messages.welcome"), "From memory");
    assert_eq!(Lang::get_fallback().as_deref(), Some("en"));
}

#[test]
fn the_facade_builds_a_translator_when_none_is_bound() {
    let app = Arc::new(Container::new());
    let _guard = Container::set_local_instance(app.clone());

    assert_eq!(
        Lang::get("passwords.token"),
        "This password reset token is invalid."
    );
    assert_eq!(Lang::get_locale(), "en");
    assert!(app.bound::<Translator>());
}
