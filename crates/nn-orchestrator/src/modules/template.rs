//! Module templates: what a module's container is, and the settings form
//! that fills it in.
//!
//! A template is a TOML file (see `templates/`). Values can use
//! placeholders, `{{server.world}}` or `{{setting.render_threads}}`, filled
//! in for each module when it starts:
//!
//! - `server.name`, `server.mc_version`, `server.world` (its `level-name`)
//! - `server.host`, `server.game_port`, `server.rcon_port`: where the server
//!   is on the module's network
//! - `module.name`
//! - `setting.<key>`: a setting's value
//!
//! Mount sources are `server:<path>` (inside the server's folder),
//! `module:<path>` (the module's folder, `modules/<name>/`, kept in backups)
//! or `cache:<path>` (`modules/<name>/cache/`, left out of backups because
//! the module makes it again).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The server's name on a module's network.
pub const SERVER_HOST: &str = "minecraft";

/// When a module runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lifecycle {
    /// Starts when the server is running; stops when it stops or sleeps.
    WithServer,
    /// Runs all the time, whatever the server is doing.
    Always,
    /// Only when started by hand.
    Manual,
}

impl Lifecycle {
    /// The stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WithServer => "with-server",
            Self::Always => "always",
            Self::Manual => "manual",
        }
    }

    /// From the stored name.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "with-server" => Some(Self::WithServer),
            "always" => Some(Self::Always),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// A module's web page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Web {
    /// The port it listens on inside its container.
    pub port: u16,
}

/// A folder given to the container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    /// `server:<path>`, `module:<path>` or `cache:<path>`.
    pub source: String,
    /// Where it appears in the container.
    pub target: String,
    /// The container can't change it.
    #[serde(default)]
    pub read_only: bool,
}

/// A file written into the module's folder before each start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    /// Inside the module's folder.
    pub path: String,
    /// What it holds (placeholders are filled in).
    pub content: String,
}

/// What kind of value a setting holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    /// On or off.
    Boolean,
    /// A whole number.
    Number,
    /// One line of text.
    Text,
    /// One of `choices`.
    Choice,
}

/// One field of a module's settings form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    /// Its name in the settings and placeholders (`setting.<key>`).
    pub key: String,
    /// What the form calls it.
    pub label: String,
    /// More about it, under the field.
    #[serde(default)]
    pub help: String,
    /// What kind of value it holds.
    pub kind: SettingKind,
    /// Its value until it's set.
    pub default: Value,
    /// A boolean that must be on, or text that can't be empty.
    #[serde(default)]
    pub required: bool,
    /// The smallest number allowed.
    #[serde(default)]
    pub min: Option<i64>,
    /// The largest number allowed.
    #[serde(default)]
    pub max: Option<i64>,
    /// What a choice can be.
    #[serde(default)]
    pub choices: Vec<String>,
}

/// A kind of module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// Short name; also the default module name.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One sentence on what it does.
    pub description: String,
    /// The version of the software, for display.
    pub version: String,
    /// The image, pinned by digest (`repo@sha256:…`).
    pub image: String,
    /// Arguments after the image's entrypoint.
    #[serde(default)]
    pub command: Vec<String>,
    /// When new modules run.
    pub lifecycle: Lifecycle,
    /// Default memory limit, MiB.
    pub memory_mb: i64,
    /// Anything to know before adding one.
    #[serde(default)]
    pub notes: String,
    /// Its web page, if it has one.
    #[serde(default)]
    pub web: Option<Web>,
    /// Environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Folders it's given.
    #[serde(default)]
    pub mounts: Vec<Mount>,
    /// Files written before each start.
    #[serde(default)]
    pub files: Vec<File>,
    /// Its settings form.
    #[serde(default)]
    pub settings: Vec<Setting>,
}

const BUILT_IN: &[&str] = &[include_str!("templates/bluemap.toml")];

/// The templates that come with NetherNode.
pub fn built_in() -> &'static [Template] {
    static TEMPLATES: std::sync::LazyLock<Vec<Template>> = std::sync::LazyLock::new(|| {
        BUILT_IN
            .iter()
            .map(|raw| parse(raw).expect("built-in module templates are valid"))
            .collect()
    });
    &TEMPLATES
}

/// A built-in template by ID.
pub fn find(id: &str) -> Option<&'static Template> {
    built_in().iter().find(|t| t.id == id)
}

/// Reads and checks a template.
pub fn parse(raw: &str) -> Result<Template, String> {
    let template: Template = toml::from_str(raw).map_err(|e| e.to_string())?;
    check(&template)?;
    Ok(template)
}

fn check(t: &Template) -> Result<(), String> {
    if !valid_name(&t.id) {
        return Err(format!(
            "{}: the ID must be lowercase letters, digits and dashes",
            t.id
        ));
    }
    let Some((_, digest)) = t.image.split_once("@sha256:") else {
        return Err(format!("{}: the image must be pinned by digest", t.id));
    };
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{}: the image digest is malformed", t.id));
    }
    // Placeholders must be ones that exist, with every setting filled in.
    let defaults: Map<String, Value> = t
        .settings
        .iter()
        .map(|s| (s.key.clone(), s.default.clone()))
        .collect();
    let context = Context::example(t, &defaults);
    let texts = t
        .env
        .values()
        .chain(t.mounts.iter().map(|m| &m.source))
        .chain(t.files.iter().map(|f| &f.content));
    for text in texts {
        render(text, &context.values).map_err(|e| format!("{}: {e}", t.id))?;
    }
    for m in &t.mounts {
        let rendered = render(&m.source, &context.values)?;
        Source::parse(&rendered).map_err(|e| format!("{}: {e}", t.id))?;
        if !m.target.starts_with('/') || m.target.contains("..") || m.target.contains(':') {
            return Err(format!(
                "{}: mount target {} must be an absolute path",
                t.id, m.target
            ));
        }
    }
    for f in &t.files {
        nn_files::RelPath::parse(&f.path)
            .map_err(|_| format!("{}: file path {} isn't inside the module", t.id, f.path))?;
    }
    for s in &t.settings {
        if !s.key.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
            return Err(format!(
                "{}: setting key {} must be lowercase and _",
                t.id, s.key
            ));
        }
        if s.kind == SettingKind::Choice && s.choices.is_empty() {
            return Err(format!("{}: {} has no choices", t.id, s.key));
        }
        // The default must be valid, apart from a required box left for the
        // person adding the module to tick.
        if !(s.required && s.kind == SettingKind::Boolean) {
            value(s, &s.default).map_err(|e| format!("{}: default of {}: {e}", t.id, s.key))?;
        }
    }
    Ok(())
}

/// Module names: what folder and container names can hold.
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
}

/// Checks one setting's value, returning its normalized form.
fn value(s: &Setting, v: &Value) -> Result<Value, String> {
    match s.kind {
        SettingKind::Boolean => {
            let on = v.as_bool().ok_or("must be on or off")?;
            if s.required && !on {
                return Err(format!("“{}” has to be on", s.label));
            }
            Ok(Value::Bool(on))
        }
        SettingKind::Number => {
            let n = v.as_i64().ok_or("must be a whole number")?;
            if s.min.is_some_and(|min| n < min) || s.max.is_some_and(|max| n > max) {
                return Err(format!(
                    "“{}” must be between {} and {}",
                    s.label,
                    s.min.unwrap_or(i64::MIN),
                    s.max.unwrap_or(i64::MAX)
                ));
            }
            Ok(Value::from(n))
        }
        SettingKind::Text => {
            let text = v.as_str().ok_or("must be text")?.trim();
            if text
                .chars()
                .any(|c| c.is_control() || c == '"' || c == '\\')
            {
                return Err(format!(
                    "“{}” can't contain quotes, backslashes or line breaks",
                    s.label
                ));
            }
            if s.required && text.is_empty() {
                return Err(format!("“{}” can't be empty", s.label));
            }
            if text.chars().count() > 200 {
                return Err(format!("“{}” is too long", s.label));
            }
            Ok(Value::String(text.to_owned()))
        }
        SettingKind::Choice => {
            let choice = v.as_str().ok_or("must be one of the choices")?;
            if !s.choices.iter().any(|c| c == choice) {
                return Err(format!(
                    "“{}” must be one of {}",
                    s.label,
                    s.choices.join(", ")
                ));
            }
            Ok(Value::String(choice.to_owned()))
        }
    }
}

/// Checks settings against a template: every setting gets a value (its
/// default when not given), and keys the template doesn't have are refused.
pub fn settings(t: &Template, given: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    if let Some(unknown) = given
        .keys()
        .find(|k| !t.settings.iter().any(|s| &s.key == *k))
    {
        return Err(format!("{} has no setting {unknown}", t.name));
    }
    let mut out = Map::new();
    for s in &t.settings {
        let v = given.get(&s.key).unwrap_or(&s.default);
        out.insert(s.key.clone(), value(s, v)?);
    }
    Ok(out)
}

/// Placeholder values for one module.
pub struct Context {
    /// Placeholder name (`server.world`) to value.
    pub values: HashMap<String, String>,
}

impl Context {
    /// For a module named `name` on `server`, whose world folder is `world`.
    pub fn new(
        server: &nn_store::Server,
        world: &str,
        name: &str,
        settings: &Map<String, Value>,
    ) -> Self {
        let mut values = HashMap::from([
            ("server.name".to_owned(), server.name.clone()),
            ("server.mc_version".to_owned(), server.mc_version.clone()),
            ("server.world".to_owned(), world.to_owned()),
            ("server.host".to_owned(), SERVER_HOST.to_owned()),
            ("server.game_port".to_owned(), "25565".to_owned()),
            ("server.rcon_port".to_owned(), "25575".to_owned()),
            ("module.name".to_owned(), name.to_owned()),
        ]);
        for (key, v) in settings {
            let text = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            values.insert(format!("setting.{key}"), text);
        }
        Self { values }
    }

    fn example(t: &Template, settings: &Map<String, Value>) -> Self {
        let mut values: HashMap<String, String> = [
            "server.name",
            "server.mc_version",
            "server.host",
            "server.game_port",
            "server.rcon_port",
            "module.name",
        ]
        .into_iter()
        .map(|k| (k.to_owned(), "x".to_owned()))
        .collect();
        values.insert("server.world".into(), "world".into());
        values.insert("module.name".into(), t.id.clone());
        for key in settings.keys() {
            values.insert(format!("setting.{key}"), "1".into());
        }
        Self { values }
    }
}

/// Fills in `{{placeholders}}`. Unknown ones are an error.
pub fn render(text: &str, values: &HashMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find("}}")
            .ok_or_else(|| "a {{ placeholder isn't closed".to_owned())?;
        let key = after[..end].trim();
        let value = values
            .get(key)
            .ok_or_else(|| format!("unknown placeholder {{{{{key}}}}}"))?;
        out.push_str(value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Where a mount comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Inside the server's folder.
    Server(nn_files::RelPath),
    /// Inside the module's folder.
    Module(nn_files::RelPath),
    /// Inside the module's cache folder.
    Cache(nn_files::RelPath),
}

impl Source {
    /// Reads `server:<path>`, `module:<path>` or `cache:<path>`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (kind, path) = s
            .split_once(':')
            .ok_or_else(|| format!("mount source {s} needs server:, module: or cache:"))?;
        let path = nn_files::RelPath::parse(path)
            .map_err(|_| format!("mount source {s} isn't a path inside its folder"))?;
        if path.as_str().is_empty() && kind != "module" {
            return Err(format!("mount source {s} can't be the whole folder"));
        }
        match kind {
            "server" => Ok(Self::Server(path)),
            "module" => Ok(Self::Module(path)),
            "cache" => Ok(Self::Cache(path)),
            _ => Err(format!("mount source {s} needs server:, module: or cache:")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_templates_are_valid() {
        let bluemap = find("bluemap").expect("BlueMap is built in");
        assert_eq!(bluemap.web, Some(Web { port: 8100 }));
        assert_eq!(bluemap.lifecycle, Lifecycle::Always);
        assert!(
            bluemap
                .mounts
                .iter()
                .any(|m| m.read_only && m.target == "/app/world")
        );
    }

    #[test]
    fn placeholders_are_filled_in_or_refused() {
        let values = HashMap::from([("server.world".to_owned(), "world".to_owned())]);
        assert_eq!(
            render("server:{{ server.world }}", &values).unwrap(),
            "server:world"
        );
        assert_eq!(
            render("no placeholders", &values).unwrap(),
            "no placeholders"
        );
        assert!(
            render("{{server.nope}}", &values)
                .unwrap_err()
                .contains("unknown")
        );
        assert!(render("{{server.world", &values).is_err());
    }

    #[test]
    fn mounts_stay_inside_their_folders() {
        assert!(matches!(
            Source::parse("server:world"),
            Ok(Source::Server(_))
        ));
        assert!(matches!(Source::parse("cache:web"), Ok(Source::Cache(_))));
        for bad in [
            "server:../other",
            "server:/etc",
            "module:../../x",
            "host:/var/run/docker.sock",
            "world",
            "server:",
        ] {
            assert!(Source::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_are_checked_and_defaulted() {
        let t = find("bluemap").unwrap();
        let none = Map::new();
        let refused = settings(t, &none).unwrap_err();
        assert!(refused.contains("has to be on"), "{refused}");
        let given = serde_json::json!({ "accept_download": true })
            .as_object()
            .unwrap()
            .clone();
        let filled = settings(t, &given).unwrap();
        assert_eq!(filled["render_threads"], 1);
        let too_many = serde_json::json!({ "accept_download": true, "render_threads": 99 });
        assert!(settings(t, too_many.as_object().unwrap()).is_err());
        let unknown = serde_json::json!({ "accept_download": true, "nope": 1 });
        assert!(
            settings(t, unknown.as_object().unwrap())
                .unwrap_err()
                .contains("no setting")
        );
    }

    #[test]
    fn templates_must_pin_their_image() {
        let raw = include_str!("templates/bluemap.toml");
        let unpinned = raw.replace("bluemap@sha256:", "bluemap:latest#");
        assert!(parse(&unpinned).unwrap_err().contains("pinned"));
        let escaping = raw.replace("module:config", "module:../../etc");
        assert!(parse(&escaping).is_err());
    }

    #[test]
    fn module_names_are_folder_safe() {
        assert!(valid_name("bluemap"));
        assert!(valid_name("map-2"));
        for bad in ["", "-x", "Map", "a.b", "a/b", &"x".repeat(33)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
