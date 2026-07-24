-- FTS5's default crisis-merge threshold is 16 segments. On a 930k-status
-- portable database a normal 24-status UPSERT crossed that threshold and
-- performed a full synchronous merge while holding Awayuki's only writer
-- connection for 28.5 seconds. WAL keeps readers concurrent with a writer,
-- but it cannot make that one writer available to streaming/API persistence.
--
-- Keep incremental automerge enabled, with a less aggressive fan-in, and push
-- the unbounded crisis merge far outside the interactive write path. Explicit
-- maintenance/benchmarks may still request a bounded merge when appropriate.
INSERT INTO status_search_fts(status_search_fts, rank)
VALUES('automerge', 8);

INSERT INTO status_search_fts(status_search_fts, rank)
VALUES('crisismerge', 128);
