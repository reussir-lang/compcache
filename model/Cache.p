// One key; two processes; SQLite transactions and OS lock transitions are atomic.
// The machine models shared resources, not a server in the implementation.
enum Status { Missing, Building, Ready, Pruning }
type Snapshot = (status: Status, complete: bool, readers: int, owner: int);
event eSnapshot: Snapshot;
event eRead: Snapshot;
event eDelete: Snapshot;
event ePublish: Snapshot;
event eRecoveryStart;
event eRecoveryDone;
event eStep;

spec Visibility observes eSnapshot, eRead, eDelete, ePublish {
  start state Check {
    on eSnapshot do (s: Snapshot) {
      assert s.status != Ready || s.complete, "Ready requires durable complete files";
      assert s.readers == 0 || s.status == Ready, "A reader must retain a Ready runtime";
      assert s.owner == -1 || s.readers == 0, "Exclusive ownership excludes readers";
    }
    on eRead do (s: Snapshot) {
      assert s.status == Ready && s.complete && s.owner == -1,
        "A reader must never observe partial publication or pruning";
    }
    on eDelete do (s: Snapshot) {
      assert s.status == Pruning && s.owner != -1 && s.readers == 0,
        "Deletion requires a committed Pruning row and exclusive lease";
    }
    on ePublish do (s: Snapshot) {
      assert s.status == Building && s.complete && s.owner != -1,
        "Publication requires completed production under the builder lease";
    }
  }
}

spec RecoveryProgress observes eRecoveryStart, eRecoveryDone {
  start state Idle { on eRecoveryStart goto Recovering; }
  hot state Recovering { on eRecoveryDone goto Idle; }
}

machine CacheModel {
  var status: Status;
  var complete: bool;
  var owner: int;
  var readers: set[int];
  var steps: int;
  var broken: bool;
  var actor: int;
  var op: int;

  fun Snap(): Snapshot {
    return (status = status, complete = complete, readers = sizeof(readers), owner = owner);
  }
  start state Init {
    entry (unsafePrune: bool) {
      broken = unsafePrune;
      owner = -1;
      status = Missing;
      send this, eStep;
    }
    on eStep do {
      if (steps == 60) { raise halt; }
      steps = steps + 1;
      actor = choose(2);
      op = choose(10);
      // Claim: the row commits before any producer starts touching files.
      if (op == 0 && owner == -1 && sizeof(readers) == 0 && status == Missing) {
        owner = actor;
        status = Building;
      }
      // Produce and sync, without publishing yet.
      if (op == 1 && owner == actor && status == Building) { complete = true; }
      // Commit Ready, then release exclusive lock.
      if (op == 2 && owner == actor && status == Building && complete) {
        announce ePublish, Snap();
        status = Ready;
        owner = -1;
      }
      // Acquire shared lock and check status in the same abstract step.
      if (op == 3 && owner == -1 && status == Ready && !(actor in readers)) {
        announce eRead, Snap();
        readers += (actor);
      }
      if (op == 4 && actor in readers) { readers -= (actor); }
      // Recheck the deadline under the exclusive lease. Fresh rows are skipped.
      if (op == 5 && owner == -1 && status == Ready && (sizeof(readers) == 0 || broken)) {
        if ($) {
          owner = actor;
          status = Pruning;
        }
      }
      if (op == 6 && owner == actor && status == Pruning) {
        announce eDelete, Snap();
        complete = false;
      }
      if (op == 7 && owner == actor && status == Pruning && !complete) {
        status = Missing;
        owner = -1;
      }
      // A crash releases OS locks, never changes durable SQLite state.
      if (op == 8) {
        if (owner == actor) { owner = -1; }
        if (actor in readers) { readers -= (actor); }
      }
      // Try-lock recovery skips live owners. Model I/O failure by leaving the
      // Pruning row intact and reporting failure, then retry in a later step.
      if (op == 9 && owner == -1 && sizeof(readers) == 0 &&
          (status == Building || status == Pruning)) {
        owner = actor;
        status = Pruning;
        announce eDelete, Snap();
        if ($) {
          complete = false;
          status = Missing;
        }
        owner = -1;
      }
      announce eSnapshot, Snap();
      send this, eStep;
    }
  }
}

// Recovery terminates once failures and competing owners cease. This is a
// conditional progress property; perpetual I/O failure cannot imply success.
machine RecoveryDriver {
  start state Init {
    entry {
      announce eRecoveryStart;
      send this, eStep;
      goto Delete;
    }
  }
  state Delete {
    on eStep do { send this, eStep; goto Finish; }
  }
  state Finish {
    on eStep do { announce eRecoveryDone; raise halt; }
  }
}
machine Main {
  start state Init { entry { new CacheModel(false); } }
}
machine UnsafeMain {
  start state Init { entry { new CacheModel(true); } }
}

test tcSafety [main=Main]: assert Visibility in { Main, CacheModel };
test tcRecovery [main=RecoveryDriver]: assert RecoveryProgress in { RecoveryDriver };
// Negative control: the checker MUST find a counterexample for this test.
test tcUnsafePrune [main=UnsafeMain]: assert Visibility in { UnsafeMain, CacheModel };
