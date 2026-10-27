#set document(title: "Append-only log policy and historical restoration")
#set page(paper: "a4", margin: 22mm)
#set text(size: 10pt)
= Append-only log policy and historical restoration
Date: 2026-10-27.

== Log policy
All plan and work logs under agentic-plan are append-only. Record every implementation, documentation, policy and verification change in a new log file or an appended entry. Preserve existing text verbatim, including dated status statements, requirements, decisions and results. Correct or supersede an earlier statement by appending a dated explanation; never rewrite the earlier entry to match the current implementation. The README may describe the current state without rewriting historical logs.

== Authorized historical restoration
Existing plan bodies had been retroactively edited as platform requirements and implementation status changed. This one-time restoration reverses those in-place edits: records 00001, 00002, 00003 and 00005 recover their exact f5eac7f bodies; record 00006 recovers its initial 366d1f8 body; record 00007 recovers its initial f43c6b3 body. Record 00004 was unchanged. Later requirements, clarifications and actual CI outcomes are retained as appended entries rather than replacing the original statements.

== Verification
The restoration script checks byte-for-byte that each restored historical body is an unchanged prefix of its corresponding current file. Later entries remain below that prefix. This restoration changes only plan logs; it does not change the cache implementation, filesystem policy or prior CI evidence.

== Pending filesystem-policy change
The current code still uses filesystem-name allowlists, including NTFS-only validation on Windows. The requested follow-up is to remove those name restrictions so local ReFS is not rejected by name, while retaining identifiable remote-storage rejection and required locking/flushing behavior. That implementation and its verification will receive a separate appended log entry.
