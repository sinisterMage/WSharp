// expect: main returning with a daemon alive
// `main` returning ends the process, even with a worker that cannot be stopped.
//
// Two promises are under test, and the case fails by *hanging* rather than by
// reporting -- which is the only way an assertion about exiting can be made.
//
// `@spawn` returns before `init` starts and the message loop is entered only
// after `init` returns, so a worker whose `init` never returns never dequeues
// anything -- including the `Stop` the exit path sends every worker. Waiting
// for one of those waits for ever, and it did: the program printed its last
// line and the process sat there until something killed it.
//
// So the exit path joins the workers that reached their loop, which keeps an
// ordinary program's exit deterministic -- a worker in the middle of a method
// still finishes -- and abandons the ones that never could. There is no timeout
// anywhere in that, which is the point of doing it this way.
//
// The daemon prints nothing: `@spawn` returns before `init` starts, so whether
// it got that far is a race. The case is the whole assertion anyway -- if this
// regresses, the suite hangs.
const forever = @import("./modules/forever.ws");

fn main() i64 {
    const w = @spawn(forever, "unused") catch return 1;
    print("main returning with a daemon alive");
    return 0;
}
