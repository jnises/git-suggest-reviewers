// #![warn(missing_debug_implementations, rust_2018_idoms)]
use anyhow::{Context, Result};
use git2::{BlameOptions, Diff, DiffFindOptions, DiffOptions, FileMode, Oid, Patch, Repository};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, info, warn};
use rayon::prelude::*;
use std::{cmp, collections::HashMap};
use structopt::StructOpt;
use thread_local::ThreadLocal;

#[derive(Debug, StructOpt)]
#[structopt(
    about = "List authors of lines changed by PR, including a few lines around the changed ones."
)]
struct Opt {
    /// Where to merge to
    base: String,

    /// Where to merge from
    compare: String,

    /// Ignore files larger than this (in bytes) to make things faster
    #[structopt(long)]
    max_file_size: Option<u64>,

    /// Verbose mode (-v, -vv, -vvv, etc), disables progress bar
    #[structopt(short, long, parse(from_occurrences))]
    verbose: usize,

    /// Don't display a progress bar
    #[structopt(long)]
    no_progress: bool,

    /// How many lines around each modification to count
    #[structopt(long, default_value = "1")]
    context: u32,

    /// Don't look further back than this when blaming files
    #[structopt(long)]
    first_commit: Option<String>,

    // Maximum number of threads. 0 is auto.
    #[structopt(long, short = "j", default_value = "0")]
    max_concurrency: usize,
}

fn get_repo() -> Result<Repository> {
    Ok(Repository::discover(".")?)
}

fn get_diff(repo: &Repository, base: Oid, compare: Oid, context: u32) -> Result<Diff<'_>> {
    let mut diff = repo.diff_tree_to_tree(
        Some(&repo.find_commit(base)?.tree()?),
        Some(&repo.find_commit(compare)?.tree()?),
        Some(
            DiffOptions::new()
                .ignore_submodules(true)
                .context_lines(context),
        ),
    )?;
    diff.find_similar(Some(DiffFindOptions::new().by_config()))?;
    Ok(diff)
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    stderrlog::new().verbosity(opt.verbose).init()?;
    rayon::ThreadPoolBuilder::new()
        .num_threads(opt.max_concurrency)
        .build_global()?;
    let progress = if opt.no_progress || opt.verbose > 0 {
        ProgressBar::hidden()
    } else {
        ProgressBar::new_spinner()
    };
    let repo = get_repo()?;
    progress.tick();
    let base = repo
        .revparse_single(&opt.base)
        .context("unable to find base")?
        .id();
    info!("base: {}", base);
    let compare = repo
        .revparse_single(&opt.compare)
        .context("unable to find compare")?
        .id();
    info!("compare: {}", compare);
    let first_commit = if let Some(first_commit) = opt.first_commit {
        let commit = repo
            .revparse_single(&first_commit)
            .context("unable to find first_commit")?
            .id();
        info!("first commit: {}", commit);
        Some(commit)
    } else {
        None
    };
    let merge_base = repo
        .merge_base(base, compare)
        .context("unable to find merge base")?;
    if let Some(commit) = first_commit {
        if let Ok(base) = repo.merge_base(merge_base, commit) {
            if base != commit {
                warn!(
                    "first_commit ({}) not an ancestor of {} and {}",
                    commit, base, compare
                );
            }
        }
    }
    info!("merge base: {:?}", merge_base);
    let diff =
        get_diff(&repo, merge_base, compare, opt.context).context("error calculating diff")?;
    progress.tick();
    progress.tick();
    let num_deltas = diff.deltas().len();
    progress.set_style(ProgressStyle::default_bar());
    progress.set_length(num_deltas as u64);
    let context = opt.context;
    let max_file_size = opt.max_file_size;
    type ModifiedMap = HashMap<(Option<String>, Option<String>), usize>;
    let repo_tls: ThreadLocal<Repository> = ThreadLocal::new();
    let diff_tls: ThreadLocal<Diff> = ThreadLocal::new();
    let modified = (0..num_deltas).into_par_iter().map(|deltaidx| -> Result<ModifiedMap> {
        let mut modified: ModifiedMap = HashMap::new();
        let repo = repo_tls.get_or_try(get_repo)?;
        let diff = diff_tls.get_or_try(|| get_diff(&repo, merge_base, compare, context))?;
        match Patch::from_diff(&diff, deltaidx) {
            Ok(Some(patch)) => {
                let delta = patch.delta();
                if delta.old_file().exists() {
                    let old_path = delta
                        .old_file()
                        .path()
                        .expect("if a file exists it should have a path");
                    if ![FileMode::Blob, FileMode::BlobExecutable]
                        .contains(&delta.old_file().mode())
                    {
                        debug!("skipping blame of {:?} because it isn't a blob", old_path);
                    } else if delta.old_file().is_binary() || delta.new_file().is_binary() {
                        debug!("skipping blame of {:?} because it is binary", old_path);
                    } else {
                        let max_size =
                            std::cmp::max(delta.old_file().size(), delta.new_file().size());
                        if max_size > max_file_size.unwrap_or(std::u64::MAX) {
                            debug!(
                                "skipping blame of {:?} because it is too large ({})",
                                old_path, max_size
                            );
                        } else {
                            if !delta.new_file().exists() {
                                info!("processing {:?} -> [deleted]", old_path);
                            } else if let Some(new_path) = delta.new_file().path() {
                                if old_path == new_path {
                                    info!("processing {:?}", old_path);
                                } else {
                                    info!("processing {:?} -> {:?}", old_path, new_path);
                                }
                            } else {
                                debug!("new new_file for {:?}", old_path);
                            }
                            let mut min_line = None;
                            let mut max_line = None;
                            for hunkidx in 0..patch.num_hunks() {
                                let (hunk, _) = patch.hunk(hunkidx)?;
                                min_line = Some(cmp::min(
                                    min_line.unwrap_or(std::u32::MAX),
                                    hunk.old_start(),
                                ));
                                max_line = Some(cmp::max(
                                    max_line.unwrap_or(std::u32::MIN),
                                    hunk.old_start() + hunk.old_lines(),
                                ));
                            }
                            let mut blame_options = BlameOptions::new();
                            blame_options
                                .newest_commit(merge_base)
                                .use_mailmap(true)
                                // not sure what this one does, but it sounds useful
                                //.track_copies_same_commit_moves(true);
                                ;
                            if let (Some(min), Some(max)) = (min_line, max_line) {
                                blame_options.min_line(min as usize).max_line(max as usize);
                            }
                            if let Some(commit) = first_commit {
                                blame_options.oldest_commit(commit);
                            }
                            match repo.blame_file(old_path, Some(&mut blame_options)) {
                                Ok(blame) => {
                                    for hunkidx in 0..patch.num_hunks() {
                                        let (hunk, _) = patch.hunk(hunkidx)?;
                                        for line in
                                            hunk.old_start()..(hunk.old_start() + hunk.old_lines())
                                        {
                                            if let Some(oldhunk) = blame.get_line(line as usize) {
                                                let sign = oldhunk.final_signature();
                                                // !!! hack to work around bug in libgit2 (?)
                                                struct HackSignature {
                                                    raw: *const std::ffi::c_void,
                                                    _owned: bool,
                                                }
                                                let signptr: &HackSignature =
                                                    unsafe { &*(&sign as *const git2::Signature as *const HackSignature) };
                                                if signptr.raw.is_null() {
                                                    warn!("bad signature found in file: {:?}. might be an author without an email or something (bug in libgit2)", old_path);
                                                } else {
                                                    let author = (
                                                        sign.name().map(|s| String::from(s)),
                                                        sign.email().map(|s| String::from(s)),
                                                    );
                                                    modified
                                                        .entry(author)
                                                        .and_modify(|e| *e += 1)
                                                        .or_insert(1);
                                                }
                                            } else {
                                                debug!(
                                                    "line {} not found in {:?}@{}",
                                                    line, old_path, merge_base
                                                );
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    debug!("error blaming {:?}: {}", old_path, e);
                                }
                            }
                        }
                    }
                } else {
                    debug!(
                        "skipping blame of {:?} because the file was created",
                        delta
                            .new_file()
                            .path()
                            .map_or("?".into(), |p| p.to_string_lossy())
                    );
                }
            }
            Err(e) => {
                warn!("error getting patch from diff: {:?}", e);
            }
            Ok(None) => {}
        }
        progress.inc(1);
        Ok(modified)
    }).try_reduce(HashMap::new, |mut acc, modified| {
        for (k, v) in modified.iter() {
            *acc.entry(k.clone()).or_insert(0) += v;
        }
        Ok(acc)
    })?;
    drop(diff_tls);
    drop(repo_tls);
    let mut modified_sorted = modified.into_iter().collect::<Vec<_>>();
    // reversed
    modified_sorted.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    progress.finish_and_clear();
    for ((name, email), lines) in modified_sorted.into_iter() {
        println!(
            "{}\t{} <{}>",
            lines,
            name.unwrap_or("?".into()),
            email.unwrap_or("?".into())
        );
    }
    Ok(())
}
