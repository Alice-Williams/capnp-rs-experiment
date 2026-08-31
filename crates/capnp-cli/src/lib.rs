//! Native `compile`, `id`, `decode`, `encode`, `eval`, and `convert` workflows.
//!
//! The compile frontend follows the pinned C++ tool's source-prefix, import
//! search, raw request, and `capnpc-*` stdin contract. Rust output is built in;
//! other plugins are child processes and receive one standard request. Text
//! tools use the same native schema frontend. JSON conversion follows the
//! pinned C++ compatibility policy; RPC tools remain later milestones.

use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use capnp_codegen::{GenerateOptions, generate_requested_file_with_options};
use capnp_compiler::request::{compile_program, emit_compiled_schema};
use capnp_compiler::semantic::{ModuleSources, ResolveLimits, ResolvedProgram};
use capnp_compiler::{Statement, StatementBody, Token, TokenKind, parse_schema};
use capnp_io::{FrameLimits, FrameRead, encode_frame, pack, parse_frame, unpack};
use capnp_json::{FormatStyle as JsonStyle, JsonCodec, JsonLimits};
use capnp_message::ReaderLimits;
use capnp_schema::{
    CapnpVersion, CompiledSchema, Node, NodeId, NodeKind, RequestedFile, SourceInfo,
};
use capnp_text::{
    EncodedMessage, FormatStyle, TextLimits, encode_structs, evaluate, evaluate_struct_message,
    format_message,
};

const COMPILER_VERSION: CapnpVersion = CapnpVersion {
    major: 2,
    minor: 0,
    micro: 0,
};

const USAGE: &str = "\
Usage:
  capnp-cli id
  capnp-cli compile --src-prefix DIR [-I DIR]... -o TARGET FILE...
  capnp-cli decode [-I DIR]... [--packed|--flat] [--short] SCHEMA TYPE
  capnp-cli encode [-I DIR]... [--packed|--flat] SCHEMA TYPE
  capnp-cli eval [-I DIR]... [--short|-b|-p|--flat] SCHEMA NAME
  capnp-cli convert [-I DIR]... [--short] FROM:TO SCHEMA TYPE

Compile targets:
  -o-                 write one binary CodeGeneratorRequest to stdout
  -orust[:DIR]        generate Rust modules (DIR defaults to current directory)
  -oPLUGIN[:DIR]      run capnpc-PLUGIN with the request on stdin

Options:
  -I, --import-path DIR       add an absolute-import search root
  --src-prefix DIR            remove DIR from requested source filenames
  --import-map ID=RUST_PATH   map an imported file ID to an external Rust module
  --packed                    read or write standard packed messages
  --flat                      read or write one unframed single-segment message
  --short                     print each text value on one line

Convert formats:
  binary, packed, flat, json
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
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    run_with_io(arguments, &mut stdin, &mut stdout)
}

/// Runs an explicit argument vector, writing normal output to `stdout`.
pub fn run(arguments: Vec<std::ffi::OsString>, stdout: &mut dyn Write) -> Result<(), CliError> {
    run_with_io(arguments, &mut io::empty(), stdout)
}

/// Runs an explicit argument vector with caller-provided standard streams.
pub fn run_with_io(
    arguments: Vec<std::ffi::OsString>,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
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
        "decode" => decode(
            parse_text_options(&arguments[1..], TextCommand::Decode)?,
            stdin,
            stdout,
        ),
        "encode" => encode(
            parse_text_options(&arguments[1..], TextCommand::Encode)?,
            stdin,
            stdout,
        ),
        "eval" => eval(
            parse_text_options(&arguments[1..], TextCommand::Eval)?,
            stdout,
        ),
        "convert" => convert(parse_convert_options(&arguments[1..])?, stdin, stdout),
        other => Err(CliError::new(format!("unknown command `{other}`\n{USAGE}"))),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConvertFormat {
    Binary,
    Packed,
    Flat,
    Json,
}

#[derive(Debug)]
struct ConvertOptions {
    import_paths: Vec<PathBuf>,
    from: ConvertFormat,
    to: ConvertFormat,
    short: bool,
    schema_file: PathBuf,
    target: String,
}

fn parse_convert_format(value: &str) -> Result<ConvertFormat, CliError> {
    match value {
        "binary" => Ok(ConvertFormat::Binary),
        "packed" => Ok(ConvertFormat::Packed),
        "flat" => Ok(ConvertFormat::Flat),
        "json" => Ok(ConvertFormat::Json),
        _ => Err(CliError::new(format!("unknown convert format `{value}`"))),
    }
}

fn parse_convert_options(arguments: &[std::ffi::OsString]) -> Result<ConvertOptions, CliError> {
    let mut import_paths = Vec::new();
    let mut short = false;
    let mut positional = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments[index]
            .to_str()
            .ok_or_else(|| CliError::new("arguments must be valid UTF-8"))?;
        if value == "-I" || value == "--import-path" {
            index += 1;
            let path = arguments
                .get(index)
                .and_then(|value| value.to_str())
                .ok_or_else(|| CliError::new(format!("{value} requires a value")))?;
            import_paths.push(PathBuf::from(path));
        } else if let Some(path) = value.strip_prefix("-I").filter(|path| !path.is_empty()) {
            import_paths.push(PathBuf::from(path));
        } else if let Some(path) = value.strip_prefix("--import-path=") {
            import_paths.push(PathBuf::from(path));
        } else if value == "--short" {
            short = true;
        } else if value == "-h" || value == "--help" {
            return Err(CliError::new(USAGE));
        } else if value.starts_with('-') {
            return Err(CliError::new(format!("unknown convert option `{value}`")));
        } else {
            positional.push(value.to_owned());
        }
        index += 1;
    }
    if positional.len() != 3 {
        return Err(CliError::new("convert requires FROM:TO SCHEMA TYPE"));
    }
    let (from, to) = positional[0]
        .split_once(':')
        .ok_or_else(|| CliError::new("convert format must be FROM:TO"))?;
    let from = parse_convert_format(from)?;
    let to = parse_convert_format(to)?;
    if from == to {
        return Err(CliError::new(
            "convert input and output formats must differ",
        ));
    }
    if from != ConvertFormat::Json && to != ConvertFormat::Json {
        return Err(CliError::new(
            "native convert currently requires JSON as an input or output format",
        ));
    }
    Ok(ConvertOptions {
        import_paths,
        from,
        to,
        short,
        schema_file: PathBuf::from(&positional[1]),
        target: positional[2].clone(),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextCommand {
    Decode,
    Encode,
    Eval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MessageMode {
    Standard,
    Packed,
    Flat,
}

#[derive(Debug)]
struct TextOptions {
    command: TextCommand,
    import_paths: Vec<PathBuf>,
    mode: MessageMode,
    binary_eval: bool,
    style: FormatStyle,
    schema_file: PathBuf,
    target: String,
}

fn parse_text_options(
    arguments: &[std::ffi::OsString],
    command: TextCommand,
) -> Result<TextOptions, CliError> {
    let mut import_paths = Vec::new();
    let mut mode = MessageMode::Standard;
    let mut binary_eval = false;
    let mut style = FormatStyle::Pretty;
    let mut positional = Vec::new();
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
            positional.extend(
                arguments[index + 1..]
                    .iter()
                    .map(|value| {
                        value
                            .to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| CliError::new("arguments must be valid UTF-8"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
            break;
        } else if value == "-I" || value == "--import-path" {
            import_paths.push(PathBuf::from(take_value(value)?));
        } else if let Some(path) = value.strip_prefix("-I").filter(|path| !path.is_empty()) {
            import_paths.push(PathBuf::from(path));
        } else if let Some(path) = value.strip_prefix("--import-path=") {
            import_paths.push(PathBuf::from(path));
        } else if value == "--packed" || value == "-p" {
            set_message_mode(&mut mode, MessageMode::Packed)?;
            binary_eval |= command == TextCommand::Eval;
        } else if value == "--flat" {
            set_message_mode(&mut mode, MessageMode::Flat)?;
            binary_eval |= command == TextCommand::Eval;
        } else if value == "-b" && command == TextCommand::Eval {
            binary_eval = true;
        } else if command == TextCommand::Eval && (value == "-o" || value == "--output") {
            parse_eval_output(&take_value(value)?, &mut mode, &mut binary_eval)?;
        } else if command == TextCommand::Eval {
            if let Some(output) = value.strip_prefix("--output=") {
                parse_eval_output(output, &mut mode, &mut binary_eval)?;
            } else if let Some(output) = value.strip_prefix("-o").filter(|value| !value.is_empty())
            {
                parse_eval_output(output, &mut mode, &mut binary_eval)?;
            } else if value == "--short" {
                style = FormatStyle::Short;
            } else if value == "-h" || value == "--help" {
                return Err(CliError::new(USAGE));
            } else if value.starts_with('-') {
                return Err(CliError::new(format!(
                    "unknown {} option `{value}`",
                    text_command_name(command)
                )));
            } else {
                positional.push(value.to_owned());
            }
        } else if value == "--short" {
            if command == TextCommand::Encode {
                return Err(CliError::new("--short is not an encode option"));
            }
            style = FormatStyle::Short;
        } else if value == "-h" || value == "--help" {
            return Err(CliError::new(USAGE));
        } else if value.starts_with('-') {
            return Err(CliError::new(format!(
                "unknown {} option `{value}`",
                text_command_name(command)
            )));
        } else {
            positional.push(value.to_owned());
        }
        index += 1;
    }
    if positional.len() != 2 {
        return Err(CliError::new(format!(
            "{} requires SCHEMA and {}",
            text_command_name(command),
            if command == TextCommand::Eval {
                "NAME"
            } else {
                "TYPE"
            }
        )));
    }
    Ok(TextOptions {
        command,
        import_paths,
        mode,
        binary_eval,
        style,
        schema_file: PathBuf::from(&positional[0]),
        target: positional[1].clone(),
    })
}

fn parse_eval_output(
    output: &str,
    mode: &mut MessageMode,
    binary: &mut bool,
) -> Result<(), CliError> {
    match output {
        "text" => {
            *mode = MessageMode::Standard;
            *binary = false;
        }
        "binary" => {
            *mode = MessageMode::Standard;
            *binary = true;
        }
        "packed" => {
            *mode = MessageMode::Packed;
            *binary = true;
        }
        "flat" => {
            *mode = MessageMode::Flat;
            *binary = true;
        }
        _ => {
            return Err(CliError::new(format!(
                "unknown eval output format `{output}`"
            )));
        }
    }
    Ok(())
}

fn set_message_mode(mode: &mut MessageMode, requested: MessageMode) -> Result<(), CliError> {
    if *mode != MessageMode::Standard && *mode != requested {
        return Err(CliError::new("--packed and --flat are mutually exclusive"));
    }
    *mode = requested;
    Ok(())
}

const fn text_command_name(command: TextCommand) -> &'static str {
    match command {
        TextCommand::Decode => "decode",
        TextCommand::Encode => "encode",
        TextCommand::Eval => "eval",
    }
}

struct ToolSchema {
    schema: Arc<CompiledSchema>,
    program: ResolvedProgram,
    entry: String,
    file_id: NodeId,
}

fn load_tool_schema(file: &Path, import_paths: &[PathBuf]) -> Result<ToolSchema, CliError> {
    let physical = fs::canonicalize(file)
        .map_err(|error| CliError::new(format!("cannot open {}: {error}", file.display())))?;
    if !physical.is_file() {
        return Err(CliError::new(format!("not a file: {}", physical.display())));
    }
    let prefix = physical
        .parent()
        .ok_or_else(|| CliError::new("schema file has no parent directory"))?
        .to_owned();
    let mut roots = vec![prefix.clone()];
    for path in import_paths {
        let path = canonical_directory(path, "import path")?;
        if !roots.contains(&path) {
            roots.push(path);
        }
    }
    let entry = rooted_path(
        physical
            .strip_prefix(&prefix)
            .map_err(|_| CliError::new("schema file escaped its parent directory"))?,
    )?;
    let mut loader = SourceLoader::new(roots);
    loader.load(entry.clone(), physical)?;
    let program = loader.sources.resolve(&entry, ResolveLimits::default());
    if !program.is_valid() {
        return Err(CliError::new(format_diagnostics(&program)));
    }
    let entries = program
        .modules
        .iter()
        .map(|module| module.path.clone())
        .collect::<Vec<_>>();
    let schema = compile_entries(&loader.sources, &entries)?;
    let file_id = program
        .module(&entry)
        .and_then(|module| module.file_id)
        .ok_or_else(|| CliError::new("compiled schema has no requested file ID"))?;
    Ok(ToolSchema {
        schema: Arc::new(schema),
        program,
        entry,
        file_id,
    })
}

fn resolve_type(schema: &ToolSchema, target: &str) -> Result<NodeId, CliError> {
    let components = target.split('.').collect::<Vec<_>>();
    if components.is_empty() || components.iter().any(|component| component.is_empty()) {
        return Err(CliError::new(format!("invalid type name `{target}`")));
    }
    let (mut scope, start) =
        imported_scope(schema, components[0]).map_or((schema.file_id, 0), |scope| (scope, 1));
    for component in &components[start..] {
        scope = schema
            .schema
            .nested(scope, component)
            .map(|node| node.id)
            .ok_or_else(|| CliError::new(format!("unknown type `{target}`")))?;
    }
    match schema.schema.node(scope).map(|node| &node.kind) {
        Some(NodeKind::Struct(_)) => Ok(scope),
        _ => Err(CliError::new(format!("`{target}` is not a struct type"))),
    }
}

fn imported_scope(schema: &ToolSchema, alias: &str) -> Option<NodeId> {
    let module = schema.program.module(&schema.entry)?;
    let binding = module
        .imports
        .iter()
        .find(|binding| binding.parent.is_none() && binding.name == alias)?;
    schema.program.module(&binding.resolved_path)?.file_id
}

fn resolve_eval_target<'a>(
    schema: &ToolSchema,
    target: &'a str,
) -> Result<(NodeId, &'a str), CliError> {
    let end = target
        .bytes()
        .position(|byte| matches!(byte, b'.' | b'['))
        .unwrap_or(target.len());
    let alias = &target[..end];
    let Some(scope) = imported_scope(schema, alias) else {
        return Ok((schema.file_id, target));
    };
    let remainder = target
        .get(end..)
        .and_then(|value| value.strip_prefix('.'))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CliError::new(format!("import alias `{alias}` needs a member name")))?;
    Ok((scope, remainder))
}

fn decode(
    options: TextOptions,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    debug_assert_eq!(options.command, TextCommand::Decode);
    let loaded = load_tool_schema(&options.schema_file, &options.import_paths)?;
    let type_id = resolve_type(&loaded, &options.target)?;
    let input = read_bounded(stdin, 256 * 1024 * 1024, "binary input")?;
    let unpacked;
    let input = if options.mode == MessageMode::Packed {
        unpacked = unpack(&input, 256 * 1024 * 1024)
            .map_err(|error| CliError::new(format!("packed input: {error}")))?;
        unpacked.as_slice()
    } else {
        input.as_slice()
    };
    if options.mode == MessageMode::Flat {
        if input.is_empty() || input.len() % 8 != 0 {
            return Err(CliError::new(
                "flat input must be a non-empty whole number of words",
            ));
        }
        write_decoded(
            &loaded.schema,
            type_id,
            [Arc::<[u8]>::from(input)],
            options.style,
            stdout,
        )?;
        return Ok(());
    }
    let mut remaining = input;
    while !remaining.is_empty() {
        let FrameRead::Message {
            frame,
            remaining: rest,
        } = parse_frame(remaining, FrameLimits::default())
            .map_err(|error| CliError::new(format!("standard frame: {error}")))?
        else {
            break;
        };
        let segments = frame
            .segments()
            .iter()
            .map(|segment| Arc::<[u8]>::from(segment.bytes()))
            .collect::<Vec<_>>();
        write_decoded(&loaded.schema, type_id, segments, options.style, stdout)?;
        remaining = rest;
    }
    Ok(())
}

fn write_decoded(
    schema: &Arc<CompiledSchema>,
    type_id: NodeId,
    segments: impl IntoIterator<Item = Arc<[u8]>>,
    style: FormatStyle,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let text = format_message(
        Arc::clone(schema),
        type_id,
        segments,
        style,
        ReaderLimits::default(),
    )
    .map_err(|error| CliError::new(format!("text decode: {error}")))?;
    writeln!(stdout, "{text}")?;
    Ok(())
}

fn encode(
    options: TextOptions,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    debug_assert_eq!(options.command, TextCommand::Encode);
    let loaded = load_tool_schema(&options.schema_file, &options.import_paths)?;
    let type_id = resolve_type(&loaded, &options.target)?;
    let input = read_bounded(stdin, TextLimits::default().max_input_bytes, "text input")?;
    let input = std::str::from_utf8(&input)
        .map_err(|error| CliError::new(format!("text input is not UTF-8: {error}")))?;
    let messages = encode_structs(&loaded.schema, type_id, input, TextLimits::default())
        .map_err(|error| CliError::new(format!("text encode: {error}")))?;
    write_encoded_messages(messages, options.mode, stdout)
}

fn write_encoded_messages(
    messages: Vec<EncodedMessage>,
    mode: MessageMode,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    if mode == MessageMode::Flat {
        if messages.len() != 1 {
            return Err(CliError::new("--flat requires exactly one input value"));
        }
        if messages[0].segments.len() != 1 {
            return Err(CliError::new(
                "flat output exceeded one segment; reduce the value size",
            ));
        }
        stdout.write_all(&messages[0].segments[0])?;
        return Ok(());
    }
    for message in messages {
        let segments = message
            .segments
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<&[u8]>>();
        let framed = encode_frame(&segments, FrameLimits::default())
            .map_err(|error| CliError::new(format!("standard frame: {error}")))?;
        if mode == MessageMode::Packed {
            let limit = framed
                .len()
                .checked_mul(2)
                .and_then(|value| value.checked_add(16))
                .ok_or_else(|| CliError::new("packed output limit overflow"))?;
            let packed = pack(&framed, limit)
                .map_err(|error| CliError::new(format!("packed output: {error}")))?;
            stdout.write_all(&packed)?;
        } else {
            stdout.write_all(&framed)?;
        }
    }
    Ok(())
}

fn eval(options: TextOptions, stdout: &mut dyn Write) -> Result<(), CliError> {
    debug_assert_eq!(options.command, TextCommand::Eval);
    let loaded = load_tool_schema(&options.schema_file, &options.import_paths)?;
    let (scope, target) = resolve_eval_target(&loaded, &options.target)?;
    if options.binary_eval {
        let message = evaluate_struct_message(
            Arc::clone(&loaded.schema),
            scope,
            target,
            TextLimits::default(),
        )
        .map_err(|error| CliError::new(format!("eval: {error}")))?;
        return write_encoded_messages(vec![message], options.mode, stdout);
    }
    let text = evaluate(Arc::clone(&loaded.schema), scope, target, options.style)
        .map_err(|error| CliError::new(format!("eval: {error}")))?;
    writeln!(stdout, "{text}")?;
    Ok(())
}

fn convert(
    options: ConvertOptions,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let loaded = load_tool_schema(&options.schema_file, &options.import_paths)?;
    let type_id = resolve_type(&loaded, &options.target)?;
    let mut codec = JsonCodec::new();
    codec.handle_by_annotation(true);
    codec.set_style(if options.short {
        JsonStyle::Compact
    } else {
        JsonStyle::Pretty
    });
    if options.from == ConvertFormat::Json {
        let input = read_bounded(stdin, JsonLimits::default().max_input_bytes, "JSON input")?;
        let input = std::str::from_utf8(&input)
            .map_err(|error| CliError::new(format!("JSON input is not UTF-8: {error}")))?;
        let messages = codec
            .decode_structs(&loaded.schema, type_id, input)
            .map_err(|error| CliError::new(format!("JSON decode: {error}")))?;
        let mode = match options.to {
            ConvertFormat::Binary => MessageMode::Standard,
            ConvertFormat::Packed => MessageMode::Packed,
            ConvertFormat::Flat => MessageMode::Flat,
            ConvertFormat::Json => unreachable!("equal formats were rejected"),
        };
        return write_encoded_messages(messages, mode, stdout);
    }

    debug_assert_eq!(options.to, ConvertFormat::Json);
    let input = read_bounded(stdin, 256 * 1024 * 1024, "binary input")?;
    let unpacked;
    let input = if options.from == ConvertFormat::Packed {
        unpacked = unpack(&input, 256 * 1024 * 1024)
            .map_err(|error| CliError::new(format!("packed input: {error}")))?;
        unpacked.as_slice()
    } else {
        input.as_slice()
    };
    if options.from == ConvertFormat::Flat {
        if input.is_empty() || input.len() % 8 != 0 {
            return Err(CliError::new(
                "flat input must be a non-empty whole number of words",
            ));
        }
        let json = codec
            .encode_message(
                Arc::clone(&loaded.schema),
                type_id,
                [Arc::<[u8]>::from(input)],
                ReaderLimits::default(),
            )
            .map_err(|error| CliError::new(format!("JSON encode: {error}")))?;
        writeln!(stdout, "{json}")?;
        return Ok(());
    }
    let mut remaining = input;
    while !remaining.is_empty() {
        let FrameRead::Message {
            frame,
            remaining: rest,
        } = parse_frame(remaining, FrameLimits::default())
            .map_err(|error| CliError::new(format!("standard frame: {error}")))?
        else {
            break;
        };
        let segments = frame
            .segments()
            .iter()
            .map(|segment| Arc::<[u8]>::from(segment.bytes()))
            .collect::<Vec<_>>();
        let json = codec
            .encode_message(
                Arc::clone(&loaded.schema),
                type_id,
                segments,
                ReaderLimits::default(),
            )
            .map_err(|error| CliError::new(format!("JSON encode: {error}")))?;
        writeln!(stdout, "{json}")?;
        remaining = rest;
    }
    Ok(())
}

fn read_bounded(
    input: &mut dyn Read,
    limit: usize,
    description: &str,
) -> Result<Vec<u8>, CliError> {
    let take_limit = u64::try_from(limit)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| CliError::new(format!("{description} limit overflow")))?;
    let mut bytes = Vec::new();
    input.take(take_limit).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(CliError::new(format!(
            "{description} exceeds {limit} bytes"
        )));
    }
    Ok(bytes)
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
            return Err(CliError::new(format_diagnostics(&program)));
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

fn format_diagnostics(program: &ResolvedProgram) -> String {
    program
        .diagnostics
        .iter()
        .map(|value| {
            format!(
                "{}:{}-{}: {}",
                value.module, value.range.start, value.range.end, value.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
                "/capnp/c++.capnp" | "/capnp/stream.capnp" | "/capnp/compat/json.capnp"
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

    #[test]
    fn decode_encode_standard_packed_and_flat_match_reference_text() {
        let root = temporary("text-round-trip");
        let schema = root.join("wire-fixture.capnp");
        fs::write(
            &schema,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../conformance/schemas/wire-fixture.capnp"
            )),
        )
        .expect("schema source");
        let reference = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/fixtures/text/wire-short.txt"
        ))
        .trim_end();
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/fixtures/cpp/",
            "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/wire-unpacked.bin"
        ));
        let mut decoded = Vec::new();
        run_with_io(
            args(&[
                "decode",
                "--short",
                schema.to_str().expect("UTF-8 path"),
                "WireFixture",
            ]),
            &mut io::Cursor::new(fixture),
            &mut decoded,
        )
        .expect("reference frame decodes");
        assert_eq!(
            String::from_utf8(decoded).expect("text output"),
            format!("{reference}\n")
        );

        for flag in [None, Some("--packed"), Some("--flat")] {
            let mut arguments = vec!["encode"];
            if let Some(flag) = flag {
                arguments.push(flag);
            }
            arguments.push(schema.to_str().expect("UTF-8 path"));
            arguments.push("WireFixture");
            let mut binary = Vec::new();
            run_with_io(
                args(&arguments),
                &mut io::Cursor::new(reference.as_bytes()),
                &mut binary,
            )
            .expect("text encodes");
            let mut decode_arguments = vec!["decode", "--short"];
            if let Some(flag) = flag {
                decode_arguments.push(flag);
            }
            decode_arguments.push(schema.to_str().expect("UTF-8 path"));
            decode_arguments.push("WireFixture");
            let mut output = Vec::new();
            run_with_io(
                args(&decode_arguments),
                &mut io::Cursor::new(binary),
                &mut output,
            )
            .expect("native binary decodes");
            assert_eq!(
                String::from_utf8(output).expect("text output"),
                format!("{reference}\n"),
                "mode {flag:?}"
            );
        }
        fs::remove_dir_all(root).expect("temporary cleanup");
    }

    #[test]
    fn eval_resolves_nested_and_import_qualified_constants() {
        let root = temporary("eval");
        let language = root.join("language-fixture.capnp");
        let imports = root.join("import-fixture.capnp");
        fs::write(
            &language,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../conformance/schemas/language-fixture.capnp"
            )),
        )
        .expect("language schema");
        fs::write(
            &imports,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../conformance/schemas/import-fixture.capnp"
            )),
        )
        .expect("import schema");
        fs::write(
            root.join("wire-fixture.capnp"),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../conformance/schemas/wire-fixture.capnp"
            )),
        )
        .expect("wire schema");

        let mut output = Vec::new();
        run(
            args(&[
                "eval",
                language.to_str().expect("UTF-8 path"),
                "LanguageFixture.primes[4]",
            ]),
            &mut output,
        )
        .expect("nested list constant evaluates");
        assert_eq!(output, b"11\n");

        output.clear();
        run(
            args(&[
                "eval",
                imports.to_str().expect("UTF-8 path"),
                "Language.LanguageFixture.sampleBox.value",
            ]),
            &mut output,
        )
        .expect("import-qualified constant evaluates");
        assert_eq!(output, b"\"constant generic struct\"\n");
        fs::remove_dir_all(root).expect("temporary cleanup");
    }

    #[test]
    fn convert_honors_json_annotations_in_both_directions() {
        let root = temporary("json-annotations");
        let schema = root.join("json-fixture.capnp");
        fs::write(
            &schema,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../conformance/schemas/json-fixture.capnp"
            )),
        )
        .expect("JSON schema");
        let json = concat!(
            "{\"external_name\":\"x\",\"encoded\":\"AAEC/v8=\",",
            "\"hexed\":\"deadbeef\",\"detail_count\":7,",
            "\"detail_display_name\":\"shown\",\"tone\":\"LOUD\",",
            "\"kind\":\"amount64\",\"value\":\"9223372036854775807\"}"
        );
        let mut binary = Vec::new();
        run_with_io(
            args(&[
                "convert",
                "--short",
                "json:binary",
                schema.to_str().expect("UTF-8 path"),
                "JsonFixture",
            ]),
            &mut io::Cursor::new(json.as_bytes()),
            &mut binary,
        )
        .expect("annotated JSON encodes");
        let mut output = Vec::new();
        run_with_io(
            args(&[
                "convert",
                "--short",
                "binary:json",
                schema.to_str().expect("UTF-8 path"),
                "JsonFixture",
            ]),
            &mut io::Cursor::new(binary),
            &mut output,
        )
        .expect("annotated JSON decodes");
        assert_eq!(
            String::from_utf8(output).expect("UTF-8 JSON"),
            format!("{json}\n")
        );
        fs::remove_dir_all(root).expect("temporary cleanup");
    }
}
