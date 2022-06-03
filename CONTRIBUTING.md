# Contributing to Arti

We welcome new contributors!  You can get in contact with us on
[our gitlab instance](https://gitlab.torproject.org/), or on the
[`\#tor-dev IRC` channel on OFTC](https://www.torproject.org/contact/).
Make sure to familiarize yourself with our
[Code of Conduct](https://gitweb.torproject.org/community/policies.git/plain/code_of_conduct.txt).

The new-account process on our gitlab instance is moderated, to reduce
spam and abuse.  (*Insert instructions for anonymous usage here*)

## Licensing notice

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

## Setting up your Development Environment

The following section is **not** an exhaustive guide, and only covers common
setup and development tasks.

**Install dependencies**

You'll need to have a working Rust environment to build the code, and a
working Git installation to fetch the code. Additionally, please install
the SQLite 3 development files and shellcheck to successfully run git hooks.

- [Rust](https://www.rust-lang.org/tools/install) note, for Windows devices
  check the
  [Other Installation Methods](https://forge.rust-lang.org/infra/other-installation-methods.html)

- [Git](https://git-scm.com/downloads) note, for Linux, macOS, and some
  Unix-like devices Git may be available via a package manager; `apt`, `brew`,
  `yum`, `pacman`, etc. Git needs to be compiled with PCRE support to allow
  the use of `git grep -P` in the git hooks. PCRE support is the default in
  some packages, but if you compile from source set `USE_LIBPCRE=YesPlease`
  when running `make` or `--with-libpcre` when running `./configure`.

- SQLite 3 development files (e.g. available via `apt install libsqlite3-dev`)
  
- For git hooks: [shellcheck](https://github.com/koalaman/shellcheck#installing)
  (used in [`maint/shellcheck_all`](./maint/shellcheck_all))

**Clone the source code**

In order to get a copy of the latest version of the arti source code:

    $ git clone https://gitlab.torproject.org/tpo/core/arti.git

This will create a new git checkout in a directory called `arti`.

**Update the source code**

To get the latest updates, you can run:

    $ git pull origin main

> Note, if you're working on a local git branch it may be wise to use `fetch`
> and `merge` options instead
>
>     $ git fetch origin
>     $ git merge origin/main
>
> Please see a good Git tutorial for more information

**Running the unit tests**

    $ cargo test --all-features

**Installing git hooks**

This repository contains some useful [git hooks](https://git-scm.com/book/en/v2/Customizing-Git-Git-Hooks)
that you might want to use to help avoid your code failing CI checks.
You can install them with

    $ cp -v maint/hooks/* .git/hooks/

**Add fork URL**

If you've created an account at `gitlab.torproject.org`, you can add a
link to your forked arti repository at:

    $ git remote add _name_ git@gitlab.torproject.org:_name_/arti.git
    $ git fetch _name_

> *Tip*: replace `_name_` in above, and following, commands to reflect your sign
> in name.
>
> *Note*: to fork this repository, or contribute to Issues and Merge Requests,
> you will need an account on our gitlab server.  If you don't have an
> account there, you can either
> [request an account](https://gitlab.onionize.space/) or
> [report a bug anonymously](https://anonticket.onionize.space/).
>
> Check the
> [Sign In](https://gitlab.torproject.org/users/sign_in?redirect_to_referer=yes)
> page for further instructions on requesting access.

**Push to fork**

    $ git push _name_ main

> Tip, to open a Merge Request navigate to the Merge Request tab for your
> account's fork; URL will be similar to the following
>
>      https://gitlab.torproject.org/_name_/arti/-/merge_requests

## Using Arti with Torbrowser

A good first step to start hacking on arti might be to hook it up with your
Tor Browser. Please note that arti is still a work in progress and hence you
should assume that it **provides no security** at the moment.

To do so, we will launch arti independently from Tor Browser. Build arti with
`cargo build --release`.  After that launch it with some basic
configuration parameters:

    $ ./target/release/arti proxy -l debug -p 9150

This will ensure that arti sets its SOCKS port on 9150. Now we need to launch
Tor Browser and instruct it to use that SOCKS port.

### Linux

    $ TOR_SKIP_LAUNCH=1 TOR_SOCKS_PORT=9150 ./start-tor-browser.desktop

### OS X

    $ TOR_SKIP_LAUNCH=1 TOR_SOCKS_PORT=9150 /path/to/Tor\ Browser/Contents/MacOS/firefox

### Windows

Create a shortcut with the `Target` set to:

    C:\Windows\System32\cmd.exe /c "SET TOR_SKIP_LAUNCH=1&& SET TOR_SOCKS_PORT=9150&& START /D ^"C:\path\to\Tor Browser\Browser^" firefox.exe"
    
and `Start in` set to:

    "C:\path\to\Tor Browser\Browser"

(You may need to adjust the actual path to wherever you have put your Tor
Browser.)

When you start Tor browser, it will give you a big red error page because
Arti isn't offering it a control port interface.  But it will still work!
Try [check.torproject.org](https://check.torproject.org/) to be sure.

The resulting Tor Browser should be using arti.  Note that onion services
won't work (Arti doesn't have them yet), and neither will any feature
depending on Tor's control-port protocol.

Enjoy hacking on arti!

## Where are some good places to start hacking?

You might want to begin by looking around the
[codebase](https://gitlab.torproject.org/tpo/core/arti/), or getting to
know our [architecture](./doc/Architecture.md).

More tests would always be great. You can look at the [coverage reports](https://tpo.pages.torproject.net/core/arti/coverage/)
to find out what parts need the more love.

Parsing more Tor document types would be neat.

More documentation examples would be great.

Improvements or bugfixes to the existing code would be great.

Improving the look and feel of the documentation would also rock.

I've made a bunch of notes throughout the document in comments with strings
like "FIXME" or "TODO".

There is a list of features that I wish other crates had in a file called
`WANT_FROM_OTHER_CRATES`.

Finally, check out
[the bugtracker](https://gitlab.torproject.org/tpo/core/arti/-/issues).
There are some tickets there labeled as
["First Contribution"](https://gitlab.torproject.org/tpo/core/arti/-/issues?scope=all&utf8=%E2%9C%93&state=opened&label_name[]=First%20Contribution):
that label means that we think they might be a good place to start out.

## Caveat haxxor: what to watch out for

Please don't assume that what you see here is good Rust: we've tried to
follow best practices, but we've been learning Rust here as we go along.
There are probably aspects of the language or its ecosystem that we're
getting wrong.

Almost nothing about this code should be taken as "final" -- I expect
that we'll need to refactor and move around a whole bunch of code, add a
bunch of APIs, split crates, merge crates, and so on.

There are some places where I am deviating from the existing Tor
protocol under the assumption that certain proposals will be
accepted.  See [Compatibility.md](./doc/Compatibility.md) for more
information.

This code does not attempt to be indistinguishable from the current Tor
implementation.

When building the docs with `cargo doc`, use `--all-features`, or you may
find broken links.  (We welcome fixes to links broken with `--all-features`.)
