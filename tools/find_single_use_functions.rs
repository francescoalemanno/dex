use proc_macro2::Span;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprCall, ExprMethodCall, ExprPath, File, ImplItemFn, ItemFn, ItemImpl,
    ItemMod, ItemUse, Type, UseTree,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DefinitionKind {
    FreeFunction,
    AssociatedFunction,
    Method,
    TraitMethod,
}

impl DefinitionKind {
    fn label(self) -> &'static str {
        match self {
            DefinitionKind::FreeFunction => "free",
            DefinitionKind::AssociatedFunction => "assoc",
            DefinitionKind::Method => "method",
            DefinitionKind::TraitMethod => "trait",
        }
    }
}

#[derive(Clone, Debug)]
struct FunctionDef {
    id: usize,
    file: PathBuf,
    line: usize,
    module_path: Vec<String>,
    name: String,
    kind: DefinitionKind,
    owner: Option<String>,
}

#[derive(Clone, Debug)]
struct ImportedSymbol {
    module_path: Vec<String>,
    name: String,
}

#[derive(Clone, Debug)]
struct PathCall {
    file: PathBuf,
    line: usize,
    module_path: Vec<String>,
    impl_owner: Option<String>,
    segments: Vec<String>,
}

#[derive(Clone, Debug)]
struct MethodCall {
    file: PathBuf,
    line: usize,
    name: String,
}

#[derive(Clone, Debug)]
struct FileAnalysis {
    imports: HashMap<String, ImportedSymbol>,
    defs: Vec<FunctionDef>,
    path_calls: Vec<PathCall>,
    method_calls: Vec<MethodCall>,
}

impl FileAnalysis {
    fn new() -> Self {
        Self {
            imports: HashMap::new(),
            defs: Vec::new(),
            path_calls: Vec::new(),
            method_calls: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct ImplContext {
    owner: Option<String>,
    trait_impl: bool,
}

struct Analyzer {
    file: PathBuf,
    module_path: Vec<String>,
    impl_context: Option<ImplContext>,
    analysis: FileAnalysis,
}

impl Analyzer {
    fn new(file: &Path) -> Self {
        let module_name = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string();
        Self {
            file: file.to_path_buf(),
            module_path: vec![module_name],
            impl_context: None,
            analysis: FileAnalysis::new(),
        }
    }

    fn line_for(span: Span) -> usize {
        span.start().line
    }

    fn has_test_attr(attrs: &[Attribute]) -> bool {
        attrs.iter().any(|attr| {
            let path = attr.path();
            if path.is_ident("test") {
                return true;
            }
            if path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "test")
            {
                return true;
            }
            if path.is_ident("cfg") || path.is_ident("cfg_attr") {
                return match &attr.meta {
                    syn::Meta::List(list) => list.tokens.to_string().contains("test"),
                    syn::Meta::Path(_) => false,
                    syn::Meta::NameValue(_) => false,
                };
            }
            false
        })
    }

    fn push_definition(
        &mut self,
        line: usize,
        name: String,
        kind: DefinitionKind,
        owner: Option<String>,
    ) {
        let id = self.analysis.defs.len();
        self.analysis.defs.push(FunctionDef {
            id,
            file: self.file.clone(),
            line,
            module_path: self.module_path.clone(),
            name,
            kind,
            owner,
        });
    }

    fn push_path_call(&mut self, path: &ExprPath) {
        let mut segments = Vec::new();
        if let Some(owner) = path
            .qself
            .as_ref()
            .and_then(|qself| simple_type_name(&qself.ty))
        {
            segments.push(owner);
            if let Some(last) = path.path.segments.last() {
                segments.push(last.ident.to_string());
            }
        } else {
            for segment in &path.path.segments {
                segments.push(segment.ident.to_string());
            }
        }
        if segments.is_empty() {
            return;
        }
        self.analysis.path_calls.push(PathCall {
            file: self.file.clone(),
            line: Self::line_for(path.span()),
            module_path: self.module_path.clone(),
            impl_owner: self.impl_context.as_ref().and_then(|ctx| ctx.owner.clone()),
            segments,
        });
    }

    fn push_method_call(&mut self, method_call: &ExprMethodCall) {
        self.analysis.method_calls.push(MethodCall {
            file: self.file.clone(),
            line: Self::line_for(method_call.span()),
            name: method_call.method.to_string(),
        });
    }
}

impl<'ast> Visit<'ast> for Analyzer {
    fn visit_item_use(&mut self, item_use: &'ast ItemUse) {
        collect_use_tree(&item_use.tree, &mut Vec::new(), &mut self.analysis.imports);
    }

    fn visit_item_mod(&mut self, item_mod: &'ast ItemMod) {
        if Self::has_test_attr(&item_mod.attrs) {
            return;
        }
        let Some((_, items)) = &item_mod.content else {
            return;
        };
        self.module_path.push(item_mod.ident.to_string());
        for item in items {
            self.visit_item(item);
        }
        self.module_path.pop();
    }

    fn visit_item_fn(&mut self, item_fn: &'ast ItemFn) {
        if Self::has_test_attr(&item_fn.attrs) {
            return;
        }
        self.push_definition(
            Self::line_for(item_fn.sig.ident.span()),
            item_fn.sig.ident.to_string(),
            DefinitionKind::FreeFunction,
            None,
        );
        visit::visit_block(self, &item_fn.block);
    }

    fn visit_item_impl(&mut self, item_impl: &'ast ItemImpl) {
        if Self::has_test_attr(&item_impl.attrs) {
            return;
        }
        let previous = self.impl_context.clone();
        self.impl_context = Some(ImplContext {
            owner: simple_type_name(&item_impl.self_ty),
            trait_impl: item_impl.trait_.is_some(),
        });
        for item in &item_impl.items {
            self.visit_impl_item(item);
        }
        self.impl_context = previous;
    }

    fn visit_impl_item_fn(&mut self, item_fn: &'ast ImplItemFn) {
        if Self::has_test_attr(&item_fn.attrs) {
            return;
        }
        let context = self.impl_context.clone().unwrap_or(ImplContext {
            owner: None,
            trait_impl: false,
        });
        let kind = if context.trait_impl {
            DefinitionKind::TraitMethod
        } else if has_receiver(&item_fn.sig) {
            DefinitionKind::Method
        } else {
            DefinitionKind::AssociatedFunction
        };
        self.push_definition(
            Self::line_for(item_fn.sig.ident.span()),
            item_fn.sig.ident.to_string(),
            kind,
            context.owner,
        );
        visit::visit_block(self, &item_fn.block);
    }

    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        if let Expr::Path(path) = &*call.func {
            self.push_path_call(path);
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_expr_method_call(&mut self, method_call: &'ast ExprMethodCall) {
        self.push_method_call(method_call);
        visit::visit_expr_method_call(self, method_call);
    }
}

#[derive(Clone, Debug)]
struct ResolvedCall {
    file: PathBuf,
    line: usize,
}

fn has_receiver(signature: &syn::Signature) -> bool {
    signature.receiver().is_some()
}

fn simple_type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        Type::Reference(reference) => simple_type_name(&reference.elem),
        Type::Group(group) => simple_type_name(&group.elem),
        Type::Paren(paren) => simple_type_name(&paren.elem),
        _ => None,
    }
}

fn collect_use_tree(
    tree: &UseTree,
    prefix: &mut Vec<String>,
    imports: &mut HashMap<String, ImportedSymbol>,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_tree(&path.tree, prefix, imports);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut full_path = prefix.clone();
            full_path.push(name.ident.to_string());
            record_import(&name.ident.to_string(), &full_path, imports);
        }
        UseTree::Rename(rename) => {
            let mut full_path = prefix.clone();
            full_path.push(rename.ident.to_string());
            record_import(&rename.rename.to_string(), &full_path, imports);
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_tree(item, prefix, imports);
            }
        }
        UseTree::Glob(_) => {}
    }
}

fn record_import(
    local_name: &str,
    full_path: &[String],
    imports: &mut HashMap<String, ImportedSymbol>,
) {
    if full_path.first().is_none_or(|first| first != "crate") || full_path.len() < 3 {
        return;
    }
    imports.insert(
        local_name.to_string(),
        ImportedSymbol {
            module_path: full_path[1..full_path.len() - 1].to_vec(),
            name: full_path[full_path.len() - 1].clone(),
        },
    );
}

fn collect_rs_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|err| format!("read_dir {}: {}", dir.display(), err))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("read_dir entry {}: {}", dir.display(), err))?;
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn parse_file(file: &Path) -> Result<FileAnalysis, String> {
    let text =
        fs::read_to_string(file).map_err(|err| format!("read {}: {}", file.display(), err))?;
    let syntax: File =
        syn::parse_file(&text).map_err(|err| format!("parse {}: {}", file.display(), err))?;
    let mut analyzer = Analyzer::new(file);
    analyzer.visit_file(&syntax);
    Ok(analyzer.analysis)
}

fn relative_to(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn main() -> Result<(), String> {
    let cwd = env::current_dir().map_err(|err| format!("current_dir: {}", err))?;
    let target_root = env::args().nth(1).unwrap_or_else(|| "src".to_string());
    let target_root = cwd.join(target_root);

    let mut files = Vec::new();
    collect_rs_files(&target_root, &mut files)?;
    files.sort();

    let mut file_analyses = HashMap::new();
    let mut defs = Vec::new();
    for file in &files {
        let analysis = parse_file(file)?;
        defs.extend(analysis.defs.iter().cloned());
        file_analyses.insert(file.clone(), analysis);
    }
    for (index, def) in defs.iter_mut().enumerate() {
        def.id = index;
    }

    let mut free_defs = HashMap::<(Vec<String>, String), Vec<usize>>::new();
    let mut owner_defs = HashMap::<(String, String), Vec<usize>>::new();
    let mut unique_method_candidates = HashMap::<String, Vec<usize>>::new();

    for def in &defs {
        match def.kind {
            DefinitionKind::FreeFunction => {
                free_defs
                    .entry((def.module_path.clone(), def.name.clone()))
                    .or_default()
                    .push(def.id);
            }
            DefinitionKind::AssociatedFunction
            | DefinitionKind::Method
            | DefinitionKind::TraitMethod => {
                if let Some(owner) = &def.owner {
                    owner_defs
                        .entry((owner.clone(), def.name.clone()))
                        .or_default()
                        .push(def.id);
                }
                if def.kind == DefinitionKind::Method || def.kind == DefinitionKind::TraitMethod {
                    unique_method_candidates
                        .entry(def.name.clone())
                        .or_default()
                        .push(def.id);
                }
            }
        }
    }

    let mut unique_method_names = HashMap::<String, usize>::new();
    for (name, ids) in unique_method_candidates {
        let concrete_ids: Vec<usize> = ids
            .into_iter()
            .filter(|id| defs[*id].kind == DefinitionKind::Method)
            .collect();
        let has_trait_method = defs
            .iter()
            .any(|def| def.name == name && def.kind == DefinitionKind::TraitMethod);
        if concrete_ids.len() == 1 && !has_trait_method {
            unique_method_names.insert(name, concrete_ids[0]);
        }
    }

    let mut calls_by_def: Vec<Vec<ResolvedCall>> = vec![Vec::new(); defs.len()];

    for analysis in file_analyses.values() {
        for call in &analysis.path_calls {
            if let Some(def_id) =
                resolve_path_call(call, &analysis.imports, &free_defs, &owner_defs)
            {
                calls_by_def[def_id].push(ResolvedCall {
                    file: call.file.clone(),
                    line: call.line,
                });
            }
        }
        for call in &analysis.method_calls {
            if let Some(def_id) = unique_method_names.get(&call.name) {
                calls_by_def[*def_id].push(ResolvedCall {
                    file: call.file.clone(),
                    line: call.line,
                });
            }
        }
    }

    let mut results = Vec::new();
    for def in &defs {
        if def.kind == DefinitionKind::TraitMethod {
            continue;
        }
        let Some(calls) = calls_by_def.get(def.id) else {
            continue;
        };
        let mut unique_calls = HashMap::<(PathBuf, usize), ResolvedCall>::new();
        for call in calls {
            unique_calls
                .entry((call.file.clone(), call.line))
                .or_insert_with(|| call.clone());
        }
        if unique_calls.len() == 1 {
            let call = unique_calls.into_values().next().unwrap();
            results.push((def.clone(), call));
        }
    }

    results.sort_by(|a, b| {
        relative_to(&cwd, &a.0.file)
            .cmp(&relative_to(&cwd, &b.0.file))
            .then(a.0.line.cmp(&b.0.line))
    });

    println!(
        "{} functions/methods with exactly one non-test explicit use in {}",
        results.len(),
        relative_to(&cwd, &target_root)
    );
    for (def, call) in results {
        let display_name = if let Some(owner) = &def.owner {
            format!("{}::{}", owner, def.name)
        } else {
            def.name.clone()
        };
        println!(
            "{}:{} [{}] {} -> {}:{}",
            relative_to(&cwd, &def.file),
            def.line,
            def.kind.label(),
            display_name,
            relative_to(&cwd, &call.file),
            call.line
        );
    }

    Ok(())
}

fn resolve_path_call(
    call: &PathCall,
    imports: &HashMap<String, ImportedSymbol>,
    free_defs: &HashMap<(Vec<String>, String), Vec<usize>>,
    owner_defs: &HashMap<(String, String), Vec<usize>>,
) -> Option<usize> {
    let segments = &call.segments;
    match segments.as_slice() {
        [name] => {
            if let Some(imported) = imports.get(name) {
                return unique_id(
                    free_defs.get(&(imported.module_path.clone(), imported.name.clone())),
                );
            }
            return unique_id(free_defs.get(&(call.module_path.clone(), name.clone())));
        }
        [first, rest @ .., name] if first == "crate" => {
            return unique_id(free_defs.get(&(rest.to_vec(), name.clone())));
        }
        [first, rest @ .., name] if first == "self" => {
            let mut module_path = call.module_path.clone();
            module_path.extend(rest.iter().cloned());
            return unique_id(free_defs.get(&(module_path, name.clone())));
        }
        [first, rest @ .., name] if first == "super" => {
            if call.module_path.is_empty() {
                return None;
            }
            let mut module_path =
                call.module_path[..call.module_path.len().saturating_sub(1)].to_vec();
            module_path.extend(rest.iter().cloned());
            return unique_id(free_defs.get(&(module_path, name.clone())));
        }
        [owner, name] if owner == "Self" => {
            let owner = call.impl_owner.as_ref()?;
            return unique_id(owner_defs.get(&(owner.clone(), name.clone())));
        }
        [owner, name] => {
            if let Some(found) = unique_id(owner_defs.get(&(owner.clone(), name.clone()))) {
                return Some(found);
            }
            return unique_id(free_defs.get(&(vec![owner.clone()], name.clone())));
        }
        [module_path @ .., name] => {
            let module_path = module_path.to_vec();
            return unique_id(free_defs.get(&(module_path, name.clone())));
        }
        [] => None,
    }
}

fn unique_id(ids: Option<&Vec<usize>>) -> Option<usize> {
    let ids = ids?;
    (ids.len() == 1).then_some(ids[0])
}
