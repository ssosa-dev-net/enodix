//! Nothing that locks the workspace is called while it is held (#70).
//!
//! `Shell::workspace` locks a `std::sync::Mutex`, which does not lock twice
//! on one thread: 0.8.0's system ssh tab held the guard and called
//! `local_start`, which locks it again, and the tab never opened while
//! every later workspace command waited for ever. This check reads the
//! shell's own source (every `.rs` file of `src-tauri/src` outside
//! `tests`) and fails on any such call, whether or not a test runs it.
//!
//! What it knows:
//! - The lock is a call of `workspace()` on the `Shell`: a value named
//!   `shell`, `self` in `impl Shell`, or `….state::<Shell>()`. Any other
//!   `….workspace()` but `profiles.workspace()` (the `Profiles`' borrowed
//!   workspace, no lock) fails the check, to be named so.
//! - A function locks the workspace if it takes the lock or calls one
//!   that does (free functions by name, `Type::method`, and methods).
//! - The workspace is held after `let guard = shell.workspace();` until
//!   the end of its block or `drop(guard)`; by a lock in a statement's
//!   temporaries (`shell.workspace().as_ref()…`, the scrutinee of a
//!   `match`, `if let` or `while let`) for that whole statement; and
//!   inside the closure given to a function that calls it while holding
//!   the lock (`read`, `edit`, `with_profiles`…).
//!
//! Calls inside macros are read as far as their arguments parse as
//! expressions (`format!`, `eprintln!`…). A closure kept in a variable and
//! called later is not followed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::Span;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Block, Expr, ExprCall, ExprClosure, ExprIf, ExprMacro, ExprMethodCall, ExprPath, FnArg,
    ImplItem, Item, Local, Macro, Pat, Signature, Stmt, StmtMacro, Token, Type,
};

/// The lock itself.
const LOCK: &str = "Shell::workspace";

/// A function of the source, by its name (`read`, `Shell::open`).
struct Function {
    name: String,
    file: String,
    /// The type of its `impl`, for `self`.
    owner: Option<String>,
    /// Its arguments' names, receiver left out.
    params: Vec<String>,
    /// Every name it binds (arguments, `let`, closures, patterns): a call
    /// or a path by one of these is not a function of the shell's.
    locals: BTreeSet<String>,
    body: Block,
}

/// Every name bound by a pattern in what is visited.
#[derive(Default)]
struct Bindings(BTreeSet<String>);

impl<'ast> Visit<'ast> for Bindings {
    fn visit_pat_ident(&mut self, pat: &'ast syn::PatIdent) {
        self.0.insert(pat.ident.to_string());
        visit::visit_pat_ident(self, pat);
    }

    fn visit_item(&mut self, _: &'ast Item) {}
}

/// A call made while the workspace is held.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Fault {
    file: String,
    line: usize,
    function: String,
    callee: String,
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}: {} calls {} while it holds the workspace, and {} locks it again",
            self.file, self.line, self.function, self.callee, self.callee
        )
    }
}

/// What the source says.
#[derive(Default)]
struct Source {
    functions: Vec<Function>,
    /// Every `impl` type's methods, by method name.
    methods: BTreeMap<String, BTreeSet<String>>,
    /// The free functions' names.
    free: BTreeSet<String>,
    /// The module names (a call `files::x` is a call of the shell's `x`).
    modules: BTreeSet<String>,
}

impl Source {
    /// `files`: (file name, text).
    fn parse(files: &[(String, String)]) -> Self {
        let mut source = Self::default();
        for (file, text) in files {
            let parsed = syn::parse_file(text).unwrap_or_else(|error| panic!("{file}: {error}"));
            if let Some(module) = Path::new(file).file_stem().and_then(|stem| stem.to_str()) {
                source.modules.insert(module.to_owned());
            }
            source.add_items(file, &parsed.items);
        }
        source
    }

    fn add_items(&mut self, file: &str, items: &[Item]) {
        for item in items {
            match item {
                Item::Fn(function) => {
                    self.free.insert(function.sig.ident.to_string());
                    self.add(file, None, &function.sig, &function.block);
                }
                Item::Impl(block) => {
                    let Type::Path(owner) = &*block.self_ty else {
                        continue;
                    };
                    let Some(owner) = owner.path.segments.last() else {
                        continue;
                    };
                    let owner = owner.ident.to_string();
                    for item in &block.items {
                        if let ImplItem::Fn(method) = item {
                            self.methods
                                .entry(method.sig.ident.to_string())
                                .or_default()
                                .insert(owner.clone());
                            self.add(file, Some(&owner), &method.sig, &method.block);
                        }
                    }
                }
                Item::Mod(module) => {
                    if let Some((_, items)) = &module.content {
                        self.modules.insert(module.ident.to_string());
                        self.add_items(file, items);
                    }
                }
                _ => {}
            }
        }
    }

    fn add(&mut self, file: &str, owner: Option<&str>, signature: &Signature, body: &Block) {
        let name = match owner {
            Some(owner) => format!("{owner}::{}", signature.ident),
            None => signature.ident.to_string(),
        };
        let params = signature
            .inputs
            .iter()
            .filter_map(|input| match input {
                FnArg::Typed(typed) => Some(match &*typed.pat {
                    Pat::Ident(ident) => ident.ident.to_string(),
                    _ => String::new(),
                }),
                FnArg::Receiver(_) => None,
            })
            .collect();
        let mut bindings = Bindings::default();
        bindings.visit_signature(signature);
        bindings.visit_block(body);
        self.functions.push(Function {
            name,
            file: file.to_owned(),
            owner: owner.map(str::to_owned),
            params,
            locals: bindings.0,
            body: body.clone(),
        });
    }

    /// What the source's functions do with the lock: which ones take it,
    /// which closures run under it, and every call made holding it.
    fn check(&self) -> Checked {
        let mut checked = Checked {
            lockers: BTreeSet::from([LOCK.to_owned()]),
            ..Checked::default()
        };
        // Until nothing changes: a function that calls a locker locks; a
        // function that calls its closure holding the lock runs it under
        // the lock.
        loop {
            let mut changed = false;
            for function in &self.functions {
                let Scan {
                    locks, held_params, ..
                } = Scan::run(self, &checked, function);
                if locks && checked.lockers.insert(function.name.clone()) {
                    changed = true;
                }
                if !held_params.is_empty() {
                    let held = checked.under_lock.entry(function.name.clone()).or_default();
                    for param in held_params {
                        changed |= held.insert(param);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for function in &self.functions {
            let Scan {
                faults, unknown, ..
            } = Scan::run(self, &checked, function);
            checked.faults.extend(faults);
            checked.unknown.extend(unknown);
        }
        checked.faults.sort();
        checked.faults.dedup();
        checked
    }
}

#[derive(Default)]
struct Checked {
    /// The functions that lock the workspace, the lock itself included.
    lockers: BTreeSet<String>,
    /// For each function that runs some of its arguments holding the lock,
    /// their positions.
    under_lock: BTreeMap<String, BTreeSet<usize>>,
    /// Every call made holding the lock of a function that locks it.
    faults: Vec<Fault>,
    /// `….workspace()` calls on something this check cannot tell is the
    /// shell.
    unknown: Vec<String>,
}

/// One function's body, read statement by statement.
struct Scan<'a> {
    source: &'a Source,
    checked: &'a Checked,
    function: &'a Function,
    /// The workspace is held here.
    held: bool,
    /// Locks this statement may take without fault: its own temporary.
    own: usize,
    /// It calls a function that locks.
    locks: bool,
    /// Its arguments it calls holding the lock, by position.
    held_params: BTreeSet<usize>,
    faults: Vec<Fault>,
    unknown: Vec<String>,
}

impl<'a> Scan<'a> {
    fn run(source: &'a Source, checked: &'a Checked, function: &'a Function) -> Self {
        let mut scan = Self {
            source,
            checked,
            function,
            held: false,
            own: 0,
            locks: false,
            held_params: BTreeSet::new(),
            faults: Vec::new(),
            unknown: Vec::new(),
        };
        scan.visit_block(&function.body);
        scan
    }

    /// Visits `visit` holding the lock if it is held already or `scope`
    /// takes it in its temporaries, which then last for all of it.
    fn scoped(&mut self, scope: &dyn Fn(&mut TopLocks), visit: impl FnOnce(&mut Self)) {
        let mut top = TopLocks {
            scan: self,
            count: 0,
        };
        scope(&mut top);
        let taken = top.count;
        let (held, own) = (self.held, self.own);
        if taken > 0 {
            // The first of its locks is its own (unless it is held
            // already); any more lock again.
            self.own = usize::from(!held);
            self.held = true;
        }
        visit(self);
        (self.held, self.own) = (held, own);
    }

    /// Visits a block of its own (a closure's, a branch's): its locks are
    /// not the enclosing statement's.
    fn nested(&mut self, visit: impl FnOnce(&mut Self)) {
        let own = std::mem::replace(&mut self.own, 0);
        visit(self);
        self.own = own;
    }

    /// The functions a call of `func` may be.
    fn callees_of_path(&self, path: &ExprPath) -> Vec<String> {
        let segments: Vec<String> = path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let Some(last) = segments.last() else {
            return Vec::new();
        };
        let first = segments[0].as_str();
        match segments.len() {
            1 if self.function.locals.contains(last) => Vec::new(),
            1 if self.source.free.contains(last) => vec![last.clone()],
            1 => Vec::new(),
            _ if ["crate", "super", "self"].contains(&first)
                || self.source.modules.contains(first) =>
            {
                if segments.len() >= 2 && self.is_type(&segments[segments.len() - 2]) {
                    vec![format!("{}::{last}", segments[segments.len() - 2])]
                } else {
                    vec![last.clone()]
                }
            }
            _ => {
                let owner = &segments[segments.len() - 2];
                let owner = match (owner.as_str(), &self.function.owner) {
                    ("Self", Some(own)) => own.clone(),
                    _ => owner.clone(),
                };
                vec![format!("{owner}::{last}")]
            }
        }
    }

    fn is_type(&self, name: &str) -> bool {
        self.source
            .methods
            .values()
            .any(|owners| owners.contains(name))
    }

    /// Whether `receiver` is the shell.
    fn is_shell(&self, receiver: &Expr) -> bool {
        match receiver {
            Expr::Paren(inner) => self.is_shell(&inner.expr),
            Expr::Reference(inner) => self.is_shell(&inner.expr),
            Expr::Unary(inner) => self.is_shell(&inner.expr),
            Expr::Path(path) => {
                let Some(name) = path.path.get_ident() else {
                    return false;
                };
                name == "shell"
                    || (name == "self" && self.function.owner.as_deref() == Some("Shell"))
            }
            Expr::MethodCall(call) => {
                matches!(call.method.to_string().as_str(), "state" | "try_state")
                    && call.turbofish.as_ref().is_some_and(|turbofish| {
                        quote_of(&turbofish.args).replace(' ', "") == "Shell"
                    })
            }
            _ => false,
        }
    }

    /// The functions a method call may be.
    fn callees_of_method(&mut self, call: &ExprMethodCall) -> Vec<String> {
        let method = call.method.to_string();
        if self.is_shell(&call.receiver) {
            return vec![format!("Shell::{method}")];
        }
        if method == "workspace" && call.args.is_empty() {
            let receiver = quote_of(&call.receiver);
            if receiver != "profiles" {
                self.unknown.push(format!(
                    "{}: {}: `{receiver}.workspace()`",
                    self.function.file, self.function.name
                ));
            }
            return Vec::new();
        }
        self.source
            .methods
            .get(&method)
            .map(|owners| {
                owners
                    .iter()
                    .filter(|owner| *owner != "Shell")
                    .map(|owner| format!("{owner}::{method}"))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A call of one of `callees` at `span`, with `args`.
    fn call(&mut self, callees: &[String], span: Span, args: &Punctuated<Expr, Token![,]>) {
        let locker = callees
            .iter()
            .find(|callee| self.checked.lockers.contains(*callee));
        if let Some(callee) = locker {
            self.locks = true;
            if self.held {
                if callee == LOCK && self.own > 0 {
                    self.own -= 1;
                } else {
                    self.faults.push(Fault {
                        file: self.function.file.clone(),
                        line: span.start().line,
                        function: self.function.name.clone(),
                        callee: callee.clone(),
                    });
                }
            }
        }
        let under: BTreeSet<usize> = callees
            .iter()
            .filter_map(|callee| self.checked.under_lock.get(callee))
            .flatten()
            .copied()
            .collect();
        for (index, arg) in args.iter().enumerate() {
            if under.contains(&index) {
                let held = std::mem::replace(&mut self.held, true);
                self.visit_expr(arg);
                self.held = held;
            } else {
                self.visit_expr(arg);
            }
        }
    }

    /// The statement `drop(name);`: the name.
    fn dropped(stmt: &Stmt) -> Option<String> {
        let Stmt::Expr(Expr::Call(call), _) = stmt else {
            return None;
        };
        let Expr::Path(func) = &*call.func else {
            return None;
        };
        let last = func.path.segments.last()?;
        if last.ident != "drop" || call.args.len() != 1 {
            return None;
        }
        match &call.args[0] {
            Expr::Path(arg) => arg.path.get_ident().map(ToString::to_string),
            _ => None,
        }
    }

    /// The statement `let guard = shell.workspace();`: the guard's name.
    fn guard_bound(&self, local: &Local) -> Option<String> {
        let init = local.init.as_ref()?;
        if init.diverge.is_some() {
            return None;
        }
        let Expr::MethodCall(call) = &*init.expr else {
            return None;
        };
        if call.method != "workspace" || !call.args.is_empty() || !self.is_shell(&call.receiver) {
            return None;
        }
        match &local.pat {
            Pat::Ident(ident) => Some(ident.ident.to_string()),
            Pat::Type(typed) => match &*typed.pat {
                Pat::Ident(ident) => Some(ident.ident.to_string()),
                _ => None,
            },
            // `let _ = …` lets it go at once.
            _ => None,
        }
    }

    /// The arguments of `mac` as expressions, where they parse as such.
    fn macro_args(mac: &Macro) -> Vec<Expr> {
        mac.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated)
            .map(|args| args.into_iter().collect())
            .unwrap_or_default()
    }
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.nested(|scan| {
            let held = scan.held;
            let mut guards: Vec<String> = Vec::new();
            for stmt in &block.stmts {
                scan.held = held || !guards.is_empty();
                if let Some(name) = Self::dropped(stmt) {
                    if let Some(at) = guards.iter().rposition(|guard| *guard == name) {
                        guards.remove(at);
                        continue;
                    }
                }
                if let Stmt::Local(local) = stmt {
                    if let Some(name) = scan.guard_bound(local) {
                        // Its lock is a fault if the workspace is held.
                        scan.visit_local(local);
                        guards.push(name);
                        continue;
                    }
                }
                scan.scoped(&|top| top.visit_stmt(stmt), |scan| scan.visit_stmt(stmt));
            }
            scan.held = held;
        });
    }

    fn visit_item(&mut self, _: &'ast Item) {
        // A function inside a function is a function of its own.
    }

    fn visit_expr_closure(&mut self, closure: &'ast ExprClosure) {
        self.nested(|scan| {
            let body = &*closure.body;
            scan.scoped(&|top| top.visit_expr(body), |scan| scan.visit_expr(body));
        });
    }

    fn visit_expr_if(&mut self, expr: &'ast ExprIf) {
        let cond = &*expr.cond;
        if contains_let(cond) {
            // `if let`: the scrutinee's temporaries last for both branches.
            self.scoped(&|top| top.visit_expr(cond), |scan| {
                scan.visit_expr(cond);
                scan.visit_block(&expr.then_branch);
                if let Some((_, other)) = &expr.else_branch {
                    scan.nested(|scan| scan.visit_expr(other));
                }
            });
        } else {
            // A condition's temporaries go before either branch runs.
            self.scoped(&|top| top.visit_expr(cond), |scan| scan.visit_expr(cond));
            self.visit_block(&expr.then_branch);
            if let Some((_, other)) = &expr.else_branch {
                self.nested(|scan| scan.visit_expr(other));
            }
        }
    }

    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        let callees = match &*call.func {
            Expr::Path(path) => {
                // One of its own arguments, called holding the lock.
                if let Some(name) = path.path.get_ident() {
                    if self.held {
                        if let Some(index) =
                            self.function.params.iter().position(|param| name == param)
                        {
                            self.held_params.insert(index);
                        }
                    }
                }
                self.callees_of_path(path)
            }
            other => {
                self.visit_expr(other);
                Vec::new()
            }
        };
        self.call(&callees, call.span(), &call.args);
    }

    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        self.visit_expr(&call.receiver);
        let callees = self.callees_of_method(call);
        self.call(&callees, call.method.span(), &call.args);
    }

    fn visit_expr_path(&mut self, path: &'ast ExprPath) {
        // A function passed by name (`.map(local_start)`) runs where it
        // is passed.
        let callees = self.callees_of_path(path);
        if !callees.is_empty() {
            self.call(&callees, path.span(), &Punctuated::new());
        }
    }

    fn visit_expr_macro(&mut self, expr: &'ast ExprMacro) {
        for arg in Self::macro_args(&expr.mac) {
            self.visit_expr(&arg);
        }
    }

    fn visit_stmt_macro(&mut self, stmt: &'ast StmtMacro) {
        for arg in Self::macro_args(&stmt.mac) {
            self.visit_expr(&arg);
        }
    }
}

/// Counts the locks a statement takes in its own temporaries: not those
/// of the blocks, closures and branches inside it.
struct TopLocks<'s, 'a> {
    scan: &'s Scan<'a>,
    count: usize,
}

impl<'ast> Visit<'ast> for TopLocks<'_, '_> {
    fn visit_block(&mut self, _: &'ast Block) {}

    fn visit_expr_closure(&mut self, _: &'ast ExprClosure) {}

    fn visit_item(&mut self, _: &'ast Item) {}

    fn visit_expr_if(&mut self, expr: &'ast ExprIf) {
        // An `if let`'s scrutinee is the `if`'s own, handled there.
        let _ = expr;
    }

    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        if call.method == "workspace" && call.args.is_empty() && self.scan.is_shell(&call.receiver)
        {
            self.count += 1;
        }
        visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_macro(&mut self, expr: &'ast ExprMacro) {
        for arg in Scan::macro_args(&expr.mac) {
            self.visit_expr(&arg);
        }
    }

    fn visit_stmt_macro(&mut self, stmt: &'ast StmtMacro) {
        for arg in Scan::macro_args(&stmt.mac) {
            self.visit_expr(&arg);
        }
    }
}

fn contains_let(expr: &Expr) -> bool {
    match expr {
        Expr::Let(_) => true,
        Expr::Binary(binary) => contains_let(&binary.left) || contains_let(&binary.right),
        Expr::Paren(inner) => contains_let(&inner.expr),
        _ => false,
    }
}

fn quote_of(tokens: &impl quote::ToTokens) -> String {
    tokens.to_token_stream().to_string()
}

/// The shell's source files, outside its tests.
fn shell_sources() -> Vec<(String, String)> {
    fn walk(folder: &Path, root: &Path, files: &mut Vec<(String, String)>) {
        let mut entries: Vec<PathBuf> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "tests") {
                    walk(&path, root, files);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let name = path.strip_prefix(root).unwrap().display().to_string();
                files.push((name, fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    files
}

fn check(files: &[(&str, &str)]) -> Checked {
    let files: Vec<(String, String)> = files
        .iter()
        .map(|(name, text)| ((*name).to_owned(), (*text).to_owned()))
        .collect();
    Source::parse(&files).check()
}

/// Who calls what holding the lock, by caller.
fn callees(checked: &Checked) -> Vec<(String, String)> {
    let mut callees: Vec<(String, String)> = checked
        .faults
        .iter()
        .map(|fault| (fault.function.clone(), fault.callee.clone()))
        .collect();
    callees.sort();
    callees
}

#[test]
fn the_shell_never_locks_the_workspace_while_it_holds_it() {
    let checked = Source::parse(&shell_sources()).check();
    assert!(
        checked.unknown.is_empty(),
        "cannot tell whether these lock the workspace; call the shell `shell`:\n{}",
        checked.unknown.join("\n")
    );
    let faults: Vec<String> = checked.faults.iter().map(ToString::to_string).collect();
    assert!(faults.is_empty(), "\n{}", faults.join("\n"));
    // The check sees what it must: helpers that lock, and helpers that
    // run their closure holding the lock.
    for locker in [
        "local_start",
        "read",
        "edit",
        "with_profiles",
        "host_made",
        "Shell::open",
    ] {
        assert!(
            checked.lockers.contains(locker),
            "{locker} locks the workspace"
        );
    }
    for (helper, closure) in [("read", 1), ("edit", 1), ("with_open_profiles", 1)] {
        assert!(
            checked
                .under_lock
                .get(helper)
                .is_some_and(|held| held.contains(&closure)),
            "{helper} runs its closure holding the workspace"
        );
    }
}

/// `local_start` as the shell has it: it locks the workspace for a moment.
const LOCAL_START: &str = r#"
fn local_start(shell: &Shell, python: &PythonSetup) -> (Option<PathBuf>, Vec<(String, String)>) {
    let root = shell
        .workspace()
        .as_ref()
        .map(|workspace| workspace.root().to_path_buf());
    (local_start_folder(root.as_deref()), Vec::new())
}

fn read<T>(shell: &Shell, query: impl FnOnce(&Workspace) -> T) -> Result<T, String> {
    let guard = shell.workspace();
    let workspace = guard.as_ref().ok_or("no workspace is open")?;
    Ok(query(workspace))
}
"#;

fn with_local_start(body: &str) -> Checked {
    check(&[("lib.rs", LOCAL_START), ("session.rs", body)])
}

#[test]
fn the_system_ssh_tab_of_0_8_0_is_caught() {
    // The arm of `session_open` as 0.8.0 had it.
    let checked = with_local_start(
        r#"
        async fn session_open(shell: State<'_, Shell>, python: State<'_, PythonSetup>) {
            let client = ask_client_algorithms(None).await;
            let guard = shell.workspace();
            let workspace = guard.as_ref().ok_or("no workspace is open")?;
            let (folder, environment) = local_start(&shell, &python);
        }
        "#,
    );
    assert_eq!(
        callees(&checked),
        [("session_open".to_owned(), "local_start".to_owned())]
    );
    assert_eq!(checked.faults[0].file, "session.rs");
    assert_eq!(checked.faults[0].line, 6);
}

#[test]
fn the_same_arm_as_fixed_passes() {
    let checked = with_local_start(
        r#"
        async fn session_open(shell: State<'_, Shell>, python: State<'_, PythonSetup>) {
            let (folder, environment) = local_start(&shell, &python);
            let guard = shell.workspace();
            let workspace = guard.as_ref().ok_or("no workspace is open")?;
        }
        "#,
    );
    assert_eq!(callees(&checked), []);
}

#[test]
fn a_guard_let_go_or_ended_with_its_block_holds_nothing() {
    let checked = with_local_start(
        r#"
        fn dropped(shell: &Shell, python: &PythonSetup) {
            let guard = shell.workspace();
            let id = guard.as_ref().map(|w| w.id());
            drop(guard);
            local_start(shell, python);
        }
        fn in_a_block(shell: &Shell, python: &PythonSetup) {
            let id = {
                let guard = shell.workspace();
                guard.as_ref().map(|w| w.id())
            };
            local_start(shell, python);
        }
        fn a_condition(shell: &Shell, python: &PythonSetup) {
            if shell.workspace().is_some() {
                local_start(shell, python);
            }
        }
        "#,
    );
    assert_eq!(callees(&checked), []);
}

#[test]
fn a_lock_in_a_statements_temporaries_holds_it_for_the_statement() {
    let checked = with_local_start(
        r#"
        fn if_let(shell: &Shell, python: &PythonSetup) {
            if let Some(workspace) = shell.workspace().as_ref() {
                local_start(shell, python);
            }
        }
        fn matched(shell: &Shell, python: &PythonSetup) {
            match shell.workspace().as_ref() {
                Some(_) => local_start(shell, python),
                None => (Vec::new(), None),
            };
        }
        fn mapped(shell: &Shell, python: &PythonSetup) {
            let found = shell.workspace().as_ref().map(|_| local_start(shell, python));
        }
        fn twice(shell: &Shell) {
            compare(shell.workspace().as_ref(), shell.workspace().as_ref());
        }
        "#,
    );
    assert_eq!(
        callees(&checked),
        [
            ("if_let".to_owned(), "local_start".to_owned()),
            ("mapped".to_owned(), "local_start".to_owned()),
            ("matched".to_owned(), "local_start".to_owned()),
            ("twice".to_owned(), "Shell::workspace".to_owned()),
        ]
    );
}

#[test]
fn a_closure_run_under_the_lock_and_helpers_of_helpers_are_followed() {
    let checked = with_local_start(
        r#"
        fn start_here(shell: &Shell, python: &PythonSetup) -> PathBuf {
            local_start(shell, python).0.unwrap()
        }
        fn in_a_closure(shell: &Shell, python: &PythonSetup) {
            read(shell, |workspace| start_here(shell, python));
        }
        fn passed_by_name(shell: &Shell) {
            let guard = shell.workspace();
            let names = guard.iter().map(start_here);
        }
        fn in_a_macro(shell: &Shell, python: &PythonSetup) {
            let guard = shell.workspace();
            eprintln!("{:?}", start_here(shell, python));
        }
        fn by_state(app: &AppHandle, python: &PythonSetup) {
            let guard = app.state::<Shell>().workspace();
            start_here(&app.state::<Shell>(), python);
        }
        "#,
    );
    assert_eq!(
        callees(&checked),
        [
            ("by_state".to_owned(), "start_here".to_owned()),
            ("in_a_closure".to_owned(), "start_here".to_owned()),
            ("in_a_macro".to_owned(), "start_here".to_owned()),
            ("passed_by_name".to_owned(), "start_here".to_owned()),
        ]
    );
}

#[test]
fn a_lock_this_check_cannot_name_fails_it() {
    let checked = check(&[(
        "lib.rs",
        r#"
        fn other(held: &Shell, profiles: &Profiles<'_>) {
            let mine = profiles.workspace();
            let guard = held.workspace();
        }
        "#,
    )]);
    assert_eq!(checked.unknown, ["lib.rs: other: `held.workspace()`"]);
}
