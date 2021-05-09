// #![warn(missing_debug_implementations, rust_2018_idoms, clippy:all)]
use anyhow::{Context, Result};
use git2::{BlameOptions, Diff, DiffFindOptions, DiffOptions, FileMode, Oid, Patch, Repository};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, info, warn};
use rayon::prelude::*;
use std::{cell::RefCell, cmp, collections::HashMap};
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

    /// Verbose mode (-v, -vv, -vvv, etc), disables progress bar
    #[structopt(short, long, parse(from_occurrences))]
    verbose: usize,

    /// Don't display a progress bar
    #[structopt(long)]
    no_progress: bool,

    /// How many lines around each modification to count
    #[structopt(long, default_value = "1")]
    context: u32,

    /// Try not to look further back than this commit when blaming files
    #[structopt(long)]
    stop_at: Option<String>,

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
        .peel_to_commit()?
        .id();
    info!("base: {}", base);
    let compare = repo
        .revparse_single(&opt.compare)
        .context("unable to find compare")?
        .peel_to_commit()?
        .id();
    info!("compare: {}", compare);
    let merge_base = repo
        .merge_base(base, compare)
        .context("unable to find merge base")?;
    info!("merge base: {:?}", merge_base);
    let stop_at = if let Some(stop_at) = opt.stop_at {
        let mut commit = repo
            .revparse_single(&stop_at)
            .context("unable to find stop_at commit")?
            .peel_to_commit()?
            .id();
        let base = repo.merge_base(merge_base, commit)?;
        if base != commit {
            warn!(
                "stop_at ({}) not an ancestor of {} and {}. using {} instead.",
                commit, merge_base, compare, base
            );
        }
        commit = base;
        info!("stopping at commit: {}", commit);
        Some(commit)
    } else {
        None
    };
    let diff =
        get_diff(&repo, merge_base, compare, opt.context).context("error calculating diff")?;
    progress.tick();
    progress.tick();
    let num_deltas = diff.deltas().len();
    progress.set_style(ProgressStyle::default_bar());
    progress.set_length(num_deltas as u64);
    let context = opt.context;
    type ModifiedMap = HashMap<(Option<String>, Option<String>), usize>;
    let repo_tls: ThreadLocal<Repository> = ThreadLocal::new();
    let diff_tls: ThreadLocal<Diff> = ThreadLocal::new();
    type MergeBaseMap = HashMap<(Oid, Oid), Option<Oid>>;
    let merge_base_tls: ThreadLocal<RefCell<MergeBaseMap>> = ThreadLocal::new();
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
                        // TODO blame each separate continuous chunk of changed lines instead?
                        if let (Some(min), Some(max)) = (min_line, max_line) {
                            blame_options.min_line(min as usize).max_line(max as usize);
                        }
                        if let Some(commit) = stop_at {
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
                                            if let Some(stop) = stop_at {
                                                let line_commit = oldhunk.final_commit_id();
                                                if line_commit == stop {
                                                    continue;
                                                }
                                                let key = (stop, line_commit);
                                                let mut map = merge_base_tls.get_or_default().borrow_mut();
                                                let base = map.entry(key).or_insert_with(|| repo.merge_base(key.0, key.1).ok());
                                                if let Some(b) = base {
                                                    if *b != stop {
                                                        // this seems to happen a lot. oldest_commit on blame options doesn't seem to do what I expected :(
                                                        continue;
                                                    }
                                                }
                                            }
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
                                                    sign.name().map(String::from),
                                                    sign.email().map(String::from),
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
    drop(merge_base_tls);
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
            name.unwrap_or_else(|| "?".into()),
            email.unwrap_or_else(|| "?".into())
        );
    }
    Ok(())
}
