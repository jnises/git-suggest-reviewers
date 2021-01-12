use anyhow::{Context, Result};
use git2::{BlameOptions, DiffFindOptions, DiffOptions, FileMode, Patch, Repository};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, info, warn};
use std::collections::HashMap;
use structopt::StructOpt;

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
    #[structopt(long, default_value = "1073741824")] // 1 MB
    max_blame_size: u64,

    /// Verbose mode (-v, -vv, -vvv, etc), disables progress bar
    #[structopt(short, long, parse(from_occurrences))]
    verbose: usize,

    /// Don't display a progress bar
    #[structopt(long)]
    no_progress: bool,

    /// How many lines around each modification to count
    #[structopt(long, default_value = "1")]
    context: u32
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    stderrlog::new().verbosity(opt.verbose).init()?;
    let progress = if opt.no_progress || opt.verbose > 0 {
        ProgressBar::hidden()
    } else {
        ProgressBar::new_spinner()
    };
    let repo = Repository::discover(".")?;
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
    let compare_tree = repo.find_commit(compare)?.tree()?;
    let merge_base = repo
        .merge_base(base, compare)
        .context("unable to find merge base")?;
    let merge_base_tree = repo.find_commit(merge_base)?.tree()?;
    info!("merge base: {:?}", merge_base);
    let mut diff = repo.diff_tree_to_tree(
        Some(&merge_base_tree),
        Some(&compare_tree),
        Some(
            DiffOptions::new()
                .ignore_submodules(true)
                .context_lines(opt.context),
        ),
    )?;
    progress.tick();
    diff.find_similar(Some(DiffFindOptions::new().by_config()))?;
    progress.tick();
    let mut modified: HashMap<(Option<String>, Option<String>), usize> = HashMap::new();
    let num_deltas = diff.deltas().len();
    progress.set_style(ProgressStyle::default_bar());
    progress.set_length(num_deltas as u64);
    for deltaidx in 0..num_deltas {
        progress.set_position(deltaidx as u64);
        match Patch::from_diff(&diff, deltaidx) {
            Ok(Some(patch)) => {
                let delta = patch.delta();
                if !delta.old_file().exists() || !delta.new_file().exists() {
                    // TODO include all lines from removed file
                    debug!(
                        "skipping blame of {:?} because the file was created or deleted",
                        delta.old_file().path()
                    );
                } else if ![FileMode::Blob, FileMode::BlobExecutable]
                    .contains(&delta.old_file().mode())
                    || ![FileMode::Blob, FileMode::BlobExecutable]
                        .contains(&delta.new_file().mode())
                {
                    debug!(
                        "skipping blame of {:?} because it isn't a blob",
                        delta.old_file().path()
                    );
                } else if delta.old_file().is_binary() || delta.new_file().is_binary() {
                    debug!(
                        "skipping blame of {:?} because it is binary",
                        delta.old_file().path()
                    );
                } else if delta.old_file().size() > opt.max_blame_size
                    || delta.new_file().size() > opt.max_blame_size
                {
                    debug!(
                        "skipping blame of {:?} because it is too large ({})",
                        delta.old_file().path(),
                        std::cmp::max(delta.old_file().size(), delta.new_file().size())
                    );
                } else {
                    if let (Some(oldp), Some(newp)) =
                        (delta.old_file().path(), delta.new_file().path())
                    {
                        if oldp == newp {
                            info!("processing {:?}", oldp);
                        } else {
                            info!("processing {:?} -> {:?}", oldp, newp);
                        }
                    }
                    let path = delta.old_file().path().unwrap(); // unwrap since we have already checked that it exists
                    match repo.blame_file(
                        path,
                        Some(
                            BlameOptions::new()
                                .newest_commit(merge_base)
                                .use_mailmap(true)
                                // not sure what this one does, but it sounds useful
                                .track_copies_same_commit_moves(true),
                        ),
                    ) {
                        Ok(blame) => {
                            for hunkidx in 0..patch.num_hunks() {
                                let (hunk, _) = patch.hunk(hunkidx)?;
                                for line in hunk.old_start()..(hunk.old_start() + hunk.old_lines())
                                {
                                    if let Some(oldhunk) = blame.get_line(line as usize) {
                                        let sign = oldhunk.final_signature();
                                        // !!!!! horrible hack to work around bug in libgit2 (?)
                                        struct HackSignature {
                                            raw: *const std::ffi::c_void,
                                            _owned: bool,
                                        }
                                        let signptr: &HackSignature = unsafe { std::mem::transmute(&sign) };
                                        if signptr.raw.is_null() {
                                            warn!("bad signature found in file: {:?}. might be an author without an email or something (bug in libgit2)", path);
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
                                            line, path, merge_base
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            debug!("error blaming {:?}: {}", path, e);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("error getting patch from diff: {:?}", e);
            }
            Ok(None) => {}
        }
    }
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
