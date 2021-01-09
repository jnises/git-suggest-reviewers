use anyhow::{anyhow, Context, Result};
use git2::{Blame, BlameOptions, DiffDelta, DiffFormat, DiffHunk, DiffLine, Oid, Repository};
use log::{debug, error, info, warn};
use std::{collections::HashMap, path::PathBuf};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(about = "List authors of lines changed by PR")]
struct Opt {
    /// where to merge to
    base: String,

    /// where to merge from
    compare: String,

    #[structopt(short, long)]
    verbose: bool,
}

fn ref_or_id(repo: &Repository, name: &str) -> Result<Oid> {
    Ok(match Oid::from_str(name) {
        Ok(oid) => oid,
        Err(_) => repo.refname_to_id(name)?,
    })
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
    let repo = Repository::open(".")?;
    let base = ref_or_id(&repo, &opt.base).context("unable to find base")?;
    let compare = ref_or_id(&repo, &opt.compare).context("unable to find compare")?;
    let compare_tree = repo.find_commit(compare)?.tree()?;
    let merge_base = repo
        .merge_base(base, compare)
        .context("unable to find merge base")?;
    let merge_base_tree = repo.find_commit(merge_base)?.tree()?;
    if opt.verbose {
        info!("merge base: {:?}", merge_base);
    }
    let diff = repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&compare_tree), None)?;
    // TODO need to do some rename detection on the diff?
    // diff.print(DiffFormat::Patch, |delta, hunk, line| {
    //     println!("{:?} {:?} {:?}", delta, hunk, line);
    //     true
    // })?;
    // println!("diff: {:?}", diff.stats()?);
    let mut modified: HashMap<(Option<String>, Option<String>), usize> = HashMap::new();
    let mut blame_cache: Option<(PathBuf, Result<Blame, git2::Error>)> = None;
    diff.foreach(
        // file_cb
        &mut |delta: DiffDelta, _| {
            info!(
                "from: {:?} to: {:?}",
                delta.old_file().path(),
                delta.new_file().path()
            );
            true
        },
        // binary_cb
        None,
        // hunk_cb
        Some(&mut |delta: DiffDelta, hunk: DiffHunk| {
            if let Some(path) = delta.old_file().path() {
                if blame_cache.is_none() || blame_cache.as_ref().unwrap().0 != path {
                    let newblame =
                        repo.blame_file(path, Some(BlameOptions::new().newest_commit(merge_base)));
                    if let Err(ref e) = newblame {
                        warn!("error blaming {:?}: {}", path, e);
                    }
                    blame_cache = Some((path.to_path_buf(), newblame));
                }
                if let Ok(blame) = &blame_cache.as_ref().unwrap().1 {
                    for line in hunk.old_start()..(hunk.old_start() + hunk.old_lines()) {
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
                            debug!("line {} not found in {:?}@{}", line, path, merge_base);
                        }
                    }
                    // let offset = line.content_offset();
                    // blame.
                    // print!("{}", String::from_utf8_lossy(line.content()));
                }
            }
            true
        }),
        // line_cb
        None,
        // Some(
        //     &mut |delta: DiffDelta, _hunk: Option<DiffHunk>, line: DiffLine| {
        //         if let Some(path) = delta.old_file().path() {
        //             if blame_cache.is_none() || blame_cache.as_ref().unwrap().0 != path {
        //                 let newblame = repo.blame_file(path, Some(BlameOptions::new().newest_commit(merge_base)));
        //                 if let Err(ref e) = newblame {
        //                     warn!("error blaming {:?}: {}", path, e);
        //                 }
        //                 blame_cache = Some((path.to_path_buf(), newblame));
        //             }
        //             if let Ok(blame) = &blame_cache.as_ref().unwrap().1 {
        //                 let offset = line.content_offset();
        //                 blame.
        //                 print!("{}", String::from_utf8_lossy(line.content()));
        //             }
        //         }
        //         true
        //     },
        // ),
    )?;
    print!("modified: {:?}", modified);
    Ok(())
}
