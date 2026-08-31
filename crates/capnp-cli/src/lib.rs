//! Native `compile` and `id` command workflows.
//!
//! The compile frontend follows the pinned C++ tool's source-prefix, import
//! search, raw request, and `capnpc-*` stdin contract. Rust output is built in;
//! other plugins are child processes and receive one standard request. Text,
//! JSON, and RPC tools remain later milestones.

use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use capnp_codegen::{GenerateOptions, generate_requested_file_with_options};
use capnp_compiler::request::{compile_program, emit_compiled_schema};
use capnp_compiler::semantic::{ModuleSources, ResolveLimits};
use capnp_compiler::{Statement, StatementBody, Token, TokenKind, parse_schema};
use capnp_schema::{CapnpVersion, CompiledSchema, Node, NodeId, RequestedFile, SourceInfo};

const COMPILER_VERSION: CapnpVersion = CapnpVersion {
    major: 2,
    minor: 0,
    micro: 0,
};

const USAGE: &str = "\
Usage:
  capnp-cli id
  capnp-cli compile --src-prefix DIR [-I DIR]... -o TARGET FILE...

Compile targets:
  -o-                 write one binary CodeGeneratorRequest to stdout
  -orust[:DIR]        generate Rust modules (DIR defaults to current directory)
  -oPLUGIN[:DIR]      run capnpc-PLUGIN with the request on stdin

Options:
  -I, --import-path DIR       add an absolute-import search root
  --src-prefix DIR            remove DIR from requested source filenames
  --import-map ID=RUST_PATH   map an imported file ID to an external Rust module
  -h, --help                  show this help
";

#[derive(Debug)]
pub struct CliError(String);

impl CliError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

impl From<io::Error> for CliError {
    fn from(error: io::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum OutputTarget {
    Raw,
    Rust(PathBuf),
    Plugin { name: String, directory: PathBuf },
}

#[derive(Debug)]
struct CompileOptions {
    source_prefix: PathBuf,
    import_paths: Vec<PathBuf>,
    output: OutputTarget,
    import_map: BTreeMap<NodeId, String>,
    files: Vec<PathBuf>,
}

/// Runs the process command line against the real filesystem and standard IO.
pub fn run_env() -> Result<(), CliError> {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let mut stdout = io::stdout().lock();
    run(arguments, &mut stdout)
}

/// Runs an explicit argument vector, writing normal output to `stdout`.
pub fn run(arguments: Vec<std::ffi::OsString>, stdout: &mut dyn Write) -> Result<(), CliError> {
    let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
        return Err(CliError::new(USAGE));
    };
    match command {
        "-h" | "--help" | "help" => stdout.write_all(USAGE.as_bytes()).map_err(Into::into),
        "id" => {
            if arguments.len() != 1 {
                return Err(CliError::new("id accepts no arguments"));
            }
            let id = generate_id()?;
            writeln!(stdout, "@0x{id:016x};")?;
            Ok(())
        }
        "compile" => compile(parse_compile_options(&arguments[1..])?, stdout),
        other => Err(CliError::new(format!("unknown command `{other}`\n{USAGE}"))),
    }
}

fn generate_id() -> Result<u64, CliError> {
    let mut entropy = fs::File::open("/dev/urandom")
        .map_err(|error| CliError::new(format!("secure system entropy is unavailable: {error}")))?;
    generate_id_from(&mut entropy)
}

fn generate_id_from(reader: &mut dyn Read) -> Result<u64, CliError> {
    let mut bytes = [0u8; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| CliError::new(format!("secure system entropy failed: {error}")))?;
    Ok(u64::from_le_bytes(bytes) | (1u64 << 63))
}

fn parse_compile_options(arguments: &[std::ffi::OsString]) -> Result<CompileOptions, CliError> {
    let mut source_prefix = None;
    let mut import_paths = Vec::new();
    let mut output = None;
    let mut import_map = BTreeMap::new();
    let mut files = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments[index]
            .to_str()
            .ok_or_else(|| CliError::new("arguments must be valid UTF-8"))?;
        let mut take_value = |option: &str| -> Result<String, CliError> {
            index += 1;
            arguments
                .get(index)
                .and_then(|value| value.to_str())
                .map(str::to_owned)
                .ok_or_else(|| CliError::new(format!("{option} requires a value")))
        };
        if value == "--" {
            files.extend(arguments[index + 1..].iter().map(PathBuf::from));
            break;
        } else if value == "-I" || value == "--import-path" {
            import_paths.push(PathBuf::from(take_value(value)?));
        } else if let Some(path) = value.strip_prefix("-I").filter(|path| !path.is_empty()) {
            import_paths.push(PathBuf::from(path));
        } else if let Some(path) = value.strip_prefix("--import-path=") {
            import_paths.push(PathBuf::from(path));
        } else if value == "--src-prefix" {
            source_prefix = Some(PathBuf::from(take_value(value)?));
        } else if let Some(path) = value.strip_prefix("--src-prefix=") {
            source_prefix = Some(PathBuf::from(path));
        } else if value == "-o" || value == "--output" {
            output = Some(parse_output(&take_value(value)?)?);
        } else if let Some(target) = value.strip_prefix("--output=") {
            output = Some(parse_output(target)?);
        } else if let Some(target) = value.strip_prefix("-o").filter(|target| !target.is_empty()) {
            output = Some(parse_output(target)?);
        } else if value == "--import-map" {
            parse_import_map(&take_value(value)?, &mut import_map)?;
        } else if let Some(mapping) = value.strip_prefix("--import-map=") {
            parse_import_map(mapping, &mut import_map)?;
        } else if value.starts_with('-') {
            return Err(CliError::new(format!("unknown compile option `{value}`")));
        } else {
            files.push(PathBuf::from(value));
        }
        index += 1;
    }
    if files.is_empty() {
        return Err(CliError::new("compile requires at least one schema file"));
    }
    let source_prefix = source_prefix.unwrap_or(env::current_dir()?);
    let output = output.ok_or_else(|| CliError::new("compile requires -o TARGET"))?;
    Ok(CompileOptions {
        source_prefix,
        import_paths,
        output,
        import_map,
        files,
    })
}

fn parse_output(value: &str) -> Result<OutputTarget, CliError> {
    if value == "-" {
        return Ok(OutputTarget::Raw);
    }
    let (name, directory) = value
        .split_once(':')
        .map_or((value, PathBuf::from(".")), |(name, path)| {
            (name, PathBuf::from(path))
        });
    if name == "rust" {
        return Ok(OutputTarget::Rust(directory));
    }
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(CliError::new(format!("invalid plugin name `{name}`")));
    }
    Ok(OutputTarget::Plugin {
        name: name.to_owned(),
        directory,
    })
}

fn parse_import_map(value: &str, mappings: &mut BTreeMap<NodeId, String>) -> Result<(), CliError> {
    let (id, path) = value
        .split_once('=')
        .ok_or_else(|| CliError::new("--import-map expects ID=RUST_PATH"))?;
    let digits = id.strip_prefix("0x").unwrap_or(id);
    let id = u64::from_str_radix(digits, 16)
        .map_err(|_| CliError::new(format!("invalid hexadecimal file ID `{id}`")))?;
    if path.is_empty() {
        return Err(CliError::new("Rust import path cannot be empty"));
    }
    if mappings.insert(id, path.to_owned()).is_some() {
        return Err(CliError::new(format!(
            "duplicate import map for 0x{id:016x}"
        )));
    }
    Ok(())
}

fn compile(options: CompileOptions, stdout: &mut dyn Write) -> Result<(), CliError> {
    let prefix = canonical_directory(&options.source_prefix, "source prefix")?;
    let mut roots = vec![prefix.clone()];
    for path in &options.import_paths {
        let path = canonical_directory(path, "import path")?;
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    let mut loader = SourceLoader::new(roots);
    let mut entries = Vec::new();
    for file in &options.files {
        let physical = fs::canonicalize(file)
            .map_err(|error| CliError::new(format!("cannot open {}: {error}", file.display())))?;
        if !physical.is_file() {
            return Err(CliError::new(format!("not a file: {}", physical.display())));
        }
        let relative = physical.strip_prefix(&prefix).map_err(|_| {
            CliError::new(format!(
                "{} is outside --src-prefix {}",
                physical.display(),
                prefix.display()
            ))
        })?;
        let virtual_path = rooted_path(relative)?;
        loader.load(virtual_path.clone(), physical)?;
        if !entries.contains(&virtual_path) {
            entries.push(virtual_path);
        }
    }
    let schema = compile_entries(&loader.sources, &entries)?;
    let request = emit_compiled_schema(&schema)
        .map_err(|error| CliError::new(format!("request serialization failed: {error}")))?;
    match options.output {
        OutputTarget::Raw => stdout.write_all(&request).map_err(Into::into),
        OutputTarget::Rust(directory) => {
            fs::create_dir_all(&directory)?;
            let generate_options = GenerateOptions {
                import_paths: options.import_map,
            };
            let mut destinations = BTreeMap::new();
            for file in schema.requested_files() {
                let generated =
                    generate_requested_file_with_options(&schema, file.id, &generate_options)
                        .map_err(|error| {
                            CliError::new(format!("Rust generation failed: {error}"))
                        })?;
                let destination = directory.join(format!("{}.rs", generated.module_name));
                if let Some(previous) =
                    destinations.insert(destination.clone(), file.filename.clone())
                {
                    return Err(CliError::new(format!(
                        "generated output collision: {} and {} both map to {}",
                        previous,
                        file.filename,
                        destination.display()
                    )));
                }
                fs::write(destination, generated.source)?;
            }
            Ok(())
        }
        OutputTarget::Plugin { name, directory } => {
            let executable = format!("capnpc-{name}");
            invoke_plugin(Path::new(&executable), &directory, &request)
        }
    }
}

fn invoke_plugin(executable: &Path, directory: &Path, request: &[u8]) -> Result<(), CliError> {
    fs::create_dir_all(directory)?;
    let mut child = Command::new(executable)
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| {
            CliError::new(format!("failed to start {}: {error}", executable.display()))
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| CliError::new("plugin stdin was unavailable"))?
        .write_all(request)?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::new(format!(
            "{} exited with status {status}",
            executable.display()
        )))
    }
}

fn compile_entries(
    sources: &ModuleSources,
    entries: &[String],
) -> Result<CompiledSchema, CliError> {
    let mut nodes = BTreeMap::<NodeId, Node>::new();
    let mut source_infos = BTreeMap::<NodeId, SourceInfo>::new();
    let mut requested_files = BTreeMap::<NodeId, RequestedFile>::new();
    for entry in entries {
        let program = sources.resolve(entry, ResolveLimits::default());
        if !program.is_valid() {
            let diagnostics = program
                .diagnostics
                .iter()
                .map(|value| {
                    format!(
                        "{}:{}-{}: {}",
                        value.module, value.range.start, value.range.end, value.message
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Err(CliError::new(diagnostics));
        }
        let schema = compile_program(&program, COMPILER_VERSION)
            .map_err(|error| CliError::new(format!("compile failed for {entry}: {error}")))?;
        merge_values(&mut nodes, schema.nodes(), |value| value.id, "node")?;
        merge_values(
            &mut source_infos,
            schema.source_infos(),
            |value| value.id,
            "source info",
        )?;
        merge_values(
            &mut requested_files,
            schema.requested_files(),
            |value| value.id,
            "requested file",
        )?;
    }
    CompiledSchema::from_parts(
        COMPILER_VERSION,
        nodes.into_values().collect(),
        source_infos.into_values().collect(),
        requested_files.into_values().collect(),
    )
    .map_err(|error| CliError::new(format!("merged request is invalid: {error}")))
}

fn merge_values<T: Clone + PartialEq>(
    output: &mut BTreeMap<NodeId, T>,
    values: &[T],
    id: impl Fn(&T) -> NodeId,
    kind: &str,
) -> Result<(), CliError> {
    for value in values {
        let key = id(value);
        if let Some(previous) = output.get(&key) {
            if previous != value {
                return Err(CliError::new(format!(
                    "conflicting {kind} values for 0x{key:016x}"
                )));
            }
        } else {
            output.insert(key, value.clone());
        }
    }
    Ok(())
}

struct SourceLoader {
    roots: Vec<PathBuf>,
    sources: ModuleSources,
    loaded: BTreeMap<String, PathBuf>,
}

impl SourceLoader {
    fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            sources: ModuleSources::default(),
            loaded: BTreeMap::new(),
        }
    }

    fn load(&mut self, virtual_path: String, physical: PathBuf) -> Result<(), CliError> {
        let physical = fs::canonicalize(&physical)?;
        if let Some(previous) = self.loaded.get(&virtual_path) {
            if previous == &physical {
                return Ok(());
            }
            return Err(CliError::new(format!(
                "virtual path {virtual_path} resolves to both {} and {}",
                previous.display(),
                physical.display()
            )));
        }
        let source = fs::read_to_string(&physical).map_err(|error| {
            CliError::new(format!("cannot read {}: {error}", physical.display()))
        })?;
        let syntax = parse_schema(source.clone().into(), Default::default());
        if !syntax.is_valid() {
            return Err(CliError::new(format!(
                "{} has syntax errors: {:?}",
                physical.display(),
                syntax.diagnostics
            )));
        }
        self.loaded.insert(virtual_path.clone(), physical.clone());
        self.sources.insert_explicit(&virtual_path, source);
        let mut imports = Vec::new();
        collect_imports(&syntax.statements, &mut imports);
        imports.sort();
        imports.dedup();
        for requested in imports {
            let resolved = resolve_virtual(&virtual_path, &requested)?;
            if self.loaded.contains_key(&resolved) {
                continue;
            }
            let candidate = self.find_import(&physical, &requested);
            if let Some(candidate) = candidate {
                self.load(resolved, candidate)?;
            } else if !matches!(
                resolved.as_str(),
                "/capnp/c++.capnp" | "/capnp/stream.capnp"
            ) {
                return Err(CliError::new(format!(
                    "{} imports `{requested}`, which was not found in any import path",
                    physical.display()
                )));
            }
        }
        Ok(())
    }

    fn find_import(&self, importer: &Path, requested: &str) -> Option<PathBuf> {
        let requested_path = requested.trim_start_matches('/');
        let mut candidates = Vec::new();
        if !requested.starts_with('/') {
            if let Some(parent) = importer.parent() {
                candidates.push(parent.join(requested));
            }
        }
        candidates.extend(self.roots.iter().map(|root| root.join(requested_path)));
        candidates.into_iter().find(|candidate| candidate.is_file())
    }
}

fn collect_imports(statements: &[Statement], output: &mut Vec<String>) {
    for statement in statements {
        collect_import_tokens(&statement.tokens, output);
        if let StatementBody::Block(children) = &statement.body {
            collect_imports(children, output);
        }
    }
}

fn collect_import_tokens(tokens: &[Token], output: &mut Vec<String>) {
    for (index, token) in tokens.iter().enumerate() {
        if matches!(&token.kind, TokenKind::Identifier(name) if name == "import") {
            if let Some(Token {
                kind: TokenKind::StringLiteral(path),
                ..
            }) = tokens.get(index + 1)
            {
                output.push(path.clone());
            }
        }
        match &token.kind {
            TokenKind::Parenthesized(items) | TokenKind::Bracketed(items) => {
                for item in items {
                    collect_import_tokens(&item.tokens, output);
                }
            }
            _ => {}
        }
    }
}

fn canonical_directory(path: &Path, description: &str) -> Result<PathBuf, CliError> {
    let path = fs::canonicalize(path).map_err(|error| {
        CliError::new(format!("invalid {description} {}: {error}", path.display()))
    })?;
    if !path.is_dir() {
        return Err(CliError::new(format!(
            "{description} is not a directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn rooted_path(path: &Path) -> Result<String, CliError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .ok_or_else(|| CliError::new("schema paths must be valid UTF-8"))?,
            ),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(CliError::new("schema path escaped its source prefix"));
            }
        }
    }
    if parts.is_empty() {
        return Err(CliError::new("schema path cannot be empty"));
    }
    Ok(format!("/{}", parts.join("/")))
}

fn resolve_virtual(importer: &str, requested: &str) -> Result<String, CliError> {
    let mut parts = if requested.starts_with('/') {
        Vec::new()
    } else {
        importer
            .trim_start_matches('/')
            .rsplit_once('/')
            .map_or(Vec::new(), |(parent, _)| {
                parent.split('/').map(str::to_owned).collect()
            })
    };
    for part in requested.trim_start_matches('/').split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(CliError::new(format!(
                        "import `{requested}` escapes the schema root"
                    )));
                }
            }
            value => parts.push(value.to_owned()),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temporary(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "capnp-cli-{name}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("temporary directory");
        path
    }

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn ids_set_the_high_bit_and_entropy_failures_surface() {
        let id = generate_id_from(&mut io::Cursor::new([1, 2, 3, 4, 5, 6, 7, 0]))
            .expect("deterministic ID");
        assert_eq!(id, 0x8007_0605_0403_0201);
        let error = generate_id_from(&mut io::Cursor::new([1, 2, 3])).expect_err("short entropy");
        assert!(error.to_string().contains("entropy failed"));
    }

    #[test]
    fn raw_multi_file_compile_loads_relative_and_absolute_imports() {
        let root = temporary("raw");
        let schemas = root.join("schemas");
        fs::create_dir(&schemas).expect("schemas directory");
        fs::write(
            schemas.join("base.capnp"),
            "@0x8000000000001001; struct Base { value @0 :UInt32; }",
        )
        .expect("base source");
        fs::write(
            schemas.join("first.capnp"),
            "@0x8000000000001002; using B = import \"base.capnp\"; struct First { base @0 :B.Base; }",
        )
        .expect("first source");
        fs::write(
            schemas.join("second.capnp"),
            "@0x8000000000001003; using B = import \"/base.capnp\"; struct Second { base @0 :B.Base; }",
        )
        .expect("second source");
        let mut output = Vec::new();
        run(
            args(&[
                "compile",
                "--src-prefix",
                schemas.to_str().expect("UTF-8 path"),
                "-I",
                schemas.to_str().expect("UTF-8 path"),
                "-o-",
                schemas.join("first.capnp").to_str().expect("UTF-8 path"),
                schemas.join("second.capnp").to_str().expect("UTF-8 path"),
            ]),
            &mut output,
        )
        .expect("raw compile");
        let schema = CompiledSchema::from_code_generator_request(&output, Default::default())
            .expect("request reloads");
        assert_eq!(schema.requested_files().len(), 2);
        assert!(
            schema
                .nodes()
                .iter()
                .any(|node| node.short_name() == Some("Base"))
        );
        fs::remove_dir_all(root).expect("temporary cleanup");
    }

    #[test]
    fn rust_output_and_external_import_mapping_are_deterministic() {
        let root = temporary("rust");
        let output = root.join("generated");
        fs::write(
            root.join("base.capnp"),
            "@0x8000000000001011; struct Base { value @0 :UInt32; }",
        )
        .expect("base source");
        fs::write(
            root.join("consumer.capnp"),
            "@0x8000000000001012; using B = import \"base.capnp\"; struct Use { base @0 :B.Base; }",
        )
        .expect("use source");
        run(
            args(&[
                "compile",
                "--src-prefix",
                root.to_str().expect("UTF-8 path"),
                &format!("-orust:{}", output.display()),
                "--import-map=8000000000001011=external_schema",
                root.join("consumer.capnp").to_str().expect("UTF-8 path"),
            ]),
            &mut Vec::new(),
        )
        .expect("Rust compile");
        let generated = fs::read_to_string(output.join("consumer.rs")).expect("generated Rust");
        assert!(generated.contains("external_schema::base"));
        fs::remove_dir_all(root).expect("temporary cleanup");
    }

    #[test]
    fn path_escape_duplicate_mappings_and_missing_plugins_fail_closed() {
        assert!(resolve_virtual("/a/main.capnp", "../../escape.capnp").is_err());
        assert!(
            parse_compile_options(&args(&[
                "--import-map=8000000000000001=a",
                "--import-map=8000000000000001=b",
                "-o-",
                "x.capnp"
            ]))
            .is_err()
        );
        let target = parse_output("not/a/plugin").expect_err("unsafe plugin name");
        assert!(target.to_string().contains("invalid plugin"));
    }

    #[cfg(unix)]
    #[test]
    fn plugin_receives_one_standard_request_on_stdin_in_the_output_directory() {
        use std::os::unix::fs::PermissionsExt;

        let root = temporary("plugin");
        let output = root.join("output");
        let plugin = root.join("capnpc-capture");
        fs::write(&plugin, "#!/bin/sh\ncat > request.bin\n").expect("plugin script");
        let mut permissions = fs::metadata(&plugin)
            .expect("plugin metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&plugin, permissions).expect("plugin executable");
        let request = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/fixtures/cpp/",
            "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
            "compiler-request-wire-fixture.bin"
        ));
        invoke_plugin(&plugin, &output, request).expect("plugin invocation");
        let captured = fs::read(output.join("request.bin")).expect("captured request");
        CompiledSchema::from_code_generator_request(&captured, Default::default())
            .expect("plugin received a complete request");
        fs::remove_dir_all(root).expect("temporary cleanup");
    }
}
