git-review-proposal
===================
Tool that proposes which reviewers to pick for a PR based on who have previously authored the lines modified by the PR.

requirements
------------
rust (https://rustup.rs/)

build
-----
`cargo build --release`

usage
-----
    USAGE:
        git-review-proposal.exe [FLAGS] [OPTIONS] <base> <compare>

    FLAGS:
        -h, --help           Prints help information
            --no-progress    Don't display a progress bar
        -V, --version        Prints version information
        -v, --verbose        Verbose mode (-v, -vv, -vvv, etc), disables progress reporting

    OPTIONS:
            --max-blame-size <max-blame-size>    Ignore files larger than this (in bytes) to make things faster [default: 1073741824]

    ARGS:
        <base>       Where to merge to
        <compare>    Where to merge from

Output will be on lines on the form
```
223 Dade <dade@example.com>
210 Kate <kate@example.com>
100 Ramon <ramon@example.com> 
```
sorted by the number of line authored by that developer.
