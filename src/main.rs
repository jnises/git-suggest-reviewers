use anyhow::{anyhow, Context, Result};
use git2::{
    Blame, BlameOptions, DiffDelta, DiffFindOptions, DiffFormat, DiffHunk, DiffLine, DiffOptions,
    FileMode, Oid, Patch, Repository,
};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info, warn};
use std::{collections::HashMap, path::PathBuf};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(
    about = "List authors of lines changed by PR, including lines around the changed ones."
)]
struct Opt {
    /// where to merge to
    base: String,

    /// where to merge from
    compare: String,

    #[structopt(long, default_value = "1073741824")] // 1 MB
    max_blame_size: u64,

    /// more printouts, disables progress reporting
    #[structopt(short, long)]
    verbose: bool,

    #[structopt(long)]
    no_progress: bool,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    env_logger::builder()
        .filter_level(if opt.verbose {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Warn
        })
        .init();
    let progress = if opt.no_progress || opt.verbose {
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
                // 3 lines context (same as default), so we get the authors of the lines around each modification
                .context_lines(3),
        ),
    )?;
    progress.tick();
    debug!("finding similar");
    diff.find_similar(Some(DiffFindOptions::new().by_config()))?;
    progress.tick();
    // TODO need to do some rename detection on the diff?
    // diff.print(DiffFormat::Patch, |delta, hunk, line| {
    //     println!("{:?} {:?} {:?}", delta, hunk, line);
    //     true
    // })?;
    // println!("diff: {:?}", diff.stats()?);
    let mut modified: HashMap<(Option<String>, Option<String>), usize> = HashMap::new();
    // let mut blame_cache: Option<(PathBuf, Result<Blame, git2::Error>)> = None;
    let num_deltas = diff.deltas().len();
    progress.set_style(ProgressStyle::default_bar());
    progress.set_length(num_deltas as u64);
    for deltaidx in 0..num_deltas {
        progress.set_position(deltaidx as u64);
        match Patch::from_diff(&diff, deltaidx) {
            Ok(Some(patch)) => {
                let delta = patch.delta();
                info!(
                    "from: {:?} to: {:?}",
                    delta.old_file().path(),
                    delta.new_file().path()
                );
                if delta.old_file().mode() != FileMode::Blob
                    || delta.new_file().mode() != FileMode::Blob
                {
                    debug!("skipping blame of {:?} because it isn't a blob", delta.old_file().path());
                } else if delta.old_file().is_binary() || delta.new_file().is_binary() {
                    debug!("skipping blame of {:?} because it is binary", delta.old_file().path());
                } else if delta.old_file().size() > opt.max_blame_size
                    || delta.new_file().size() > opt.max_blame_size
                {
                    debug!(
                        "skipping blame of {:?} because it is too large ({})",
                        delta.old_file().path(),
                        std::cmp::max(delta.old_file().size(), delta.new_file().size())
                    );
                } else if !delta.old_file().exists() || !delta.new_file().exists() {
                    debug!(
                        "skipping blame of {:?} because the file was created or deleted",
                        delta.old_file().path()
                    );
                } else {
                    let path = delta.old_file().path().unwrap(); // unwrap since we have already checked that it exists
                    debug!("blaming {:?}", path);
                    match repo.blame_file(
                        path,
                        Some(
                            BlameOptions::new()
                                .newest_commit(merge_base)
                                .use_mailmap(true),
                        ),
                    ) {
                        Ok(blame) => {
                            debug!("done blaming");
                            let pathbuf = path.to_path_buf();
                            for hunkidx in 0..patch.num_hunks() {
                                let (hunk, _) = patch.hunk(hunkidx)?;
                                debug!("hunk: {:?}", hunk);
                                for line in hunk.old_start()..(hunk.old_start() + hunk.old_lines())
                                {
                                    if let Some(oldhunk) = blame.get_line(line as usize) {
                                        let sign = oldhunk.final_signature();
                                        let author = (
                                            sign.name().map(|s| String::from(s)),
                                            sign.email().map(|s| String::from(s)),
                                        );
                                        modified.entry(author).and_modify(|e| *e += 1).or_insert(1);
                                    // let commit_oid = oldhunk.final_commit_id();
                                    // if let Ok(commit) = repo.find_commit(commit_oid) {
                                    //     commit.author()
                                    // } else {
                                    //     warn!("cannot find commit with id {}", commit_oid);
                                    // }
                                    } else {
                                        debug!(
                                            "line {} not found in {:?}@{}",
                                            line, path, merge_base
                                        );
                                    }
                                }
                                // let offset = line.content_offset();
                                // blame.
                                // print!("{}", String::from_utf8_lossy(line.content()));
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
    // diff.foreach(
    //     // file_cb
    //     &mut |delta: DiffDelta, p: f32| {
    //         progress.set_position(50 + (p * 1000f32) as u64);
    //         debug!("p: {}", p);
    //         info!(
    //             "from: {:?} to: {:?}",
    //             delta.old_file().path(),
    //             delta.new_file().path()
    //         );
    //         true
    //     },
    //     // binary_cb
    //     None,
    //     // hunk_cb
    //     Some(&mut |delta: DiffDelta, hunk: DiffHunk| {
    //         debug!("hunk: {:?}", hunk);
    //         if let Some(path) = delta.old_file().path() {
    //             let pathbuf = path.to_path_buf();
    //             if blame_cache.is_none() || blame_cache.as_ref().unwrap().0 != pathbuf {
    //                 blame_cache =
    //                     Some((pathbuf.clone(), Err(git2::Error::from_str("not created"))));
    //                 //debug!("delta old mode: {:?}. new mode: {:?}", delta.old_file().mode(), delta.new_file().mode());
    //                 if delta.old_file().mode() != FileMode::Blob
    //                     || delta.new_file().mode() != FileMode::Blob
    //                 {
    //                     debug!("skipping blame of {:?} because it isn't a blob", path);
    //                 } else if delta.old_file().is_binary() || delta.new_file().is_binary() {
    //                     debug!("skipping blame of {:?} because it is binary", path);
    //                 } else if delta.old_file().size() > opt.max_blame_size
    //                     || delta.new_file().size() > opt.max_blame_size
    //                 {
    //                     debug!(
    //                         "skipping blame of {:?} because it is too large ({})",
    //                         path,
    //                         std::cmp::max(delta.old_file().size(), delta.new_file().size())
    //                     );
    //                 } else if !delta.old_file().exists() || !delta.new_file().exists() {
    //                     debug!(
    //                         "skipping blame of {:?} because the file was created or deleted",
    //                         path
    //                     );
    //                 } else {
    //                     debug!("blaming {:?}", path);
    //                     let newblame = repo.blame_file(
    //                         path,
    //                         Some(
    //                             BlameOptions::new()
    //                                 .newest_commit(merge_base)
    //                                 .use_mailmap(true),
    //                         ),
    //                     );
    //                     debug!("done blaming");
    //                     if let Err(ref e) = newblame {
    //                         // this happens if this is a new file. should detect that..
    //                         debug!("error blaming {:?}: {}", path, e);
    //                     }
    //                     blame_cache = Some((path.to_path_buf(), newblame));
    //                 }
    //             }
    //             if let Ok(blame) = &blame_cache.as_ref().unwrap().1 {
    //                 for line in hunk.old_start()..(hunk.old_start() + hunk.old_lines()) {
    //                     if let Some(oldhunk) = blame.get_line(line as usize) {
    //                         let sign = oldhunk.final_signature();
    //                         let author = (
    //                             sign.name().map(|s| String::from(s)),
    //                             sign.email().map(|s| String::from(s)),
    //                         );
    //                         modified.entry(author).and_modify(|e| *e += 1).or_insert(1);
    //                     // let commit_oid = oldhunk.final_commit_id();
    //                     // if let Ok(commit) = repo.find_commit(commit_oid) {
    //                     //     commit.author()
    //                     // } else {
    //                     //     warn!("cannot find commit with id {}", commit_oid);
    //                     // }
    //                     } else {
    //                         debug!("line {} not found in {:?}@{}", line, path, merge_base);
    //                     }
    //                 }
    //                 // let offset = line.content_offset();
    //                 // blame.
    //                 // print!("{}", String::from_utf8_lossy(line.content()));
    //             }
    //         }
    //         true
    //     }),
    //     // line_cb
    //     None,
    // )?;
    debug!("done!");
    let mut modified_sorted = modified.into_iter().collect::<Vec<_>>();
    // reversed
    modified_sorted.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    progress.finish();
    for ((name, email), lines) in modified_sorted.into_iter() {
        println!(
            "{}\t{} ({})",
            lines,
            name.unwrap_or("?".into()),
            email.unwrap_or("?".into())
        );
    }
    Ok(())
}
