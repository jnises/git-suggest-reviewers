# git-suggest-reviewers

Tool that suggests which reviewers to pick for a PR based on who have previously authored the lines modified by the PR.

## requirements

rust (https://rustup.rs/)

## build

`cargo build --release`

## usage

    USAGE:
        git-suggest-reviewers [FLAGS] [OPTIONS] <base> <compare>

    FLAGS:
        -h, --help           Prints help information
            --no-progress    Don't display a progress bar
        -V, --version        Prints version information
        -v, --verbose        Verbose mode (-v, -vv, -vvv, etc), disables progress bar

    OPTIONS:
            --context <context>                    How many lines around each modification to count [default: 1]
        -j, --max-concurrency <max-concurrency>     [default: 0]
            --stop-at <stop-at>                    Try not to look further back than this commit when blaming files

    ARGS:
        <base>       Where to merge to
        <compare>    Where to merge from

Output will be on lines on the form

```
223 Dade <dade@example.com>
210 Kate <kate@example.com>
100 Ramon <ramon@example.com>
```

sorted by the number of lines authored by that developer.

## known issues

Built using libgit2, so only supports repos that that library can handle.
