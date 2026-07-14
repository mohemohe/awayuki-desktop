import React from "react";
import { Loader2 } from "lucide-react";
import { t } from "../../i18n";
import type { PollSummary, TimelineStatus } from "../../types/app";
import { CustomEmojiText } from "../../components/common/CustomEmoji";
import { formatNumber } from "../../utils/format";

export function StatusPoll({
  status,
  votingSupported,
  onVote,
}: {
  status: TimelineStatus;
  votingSupported: boolean;
  onVote: (choices: number[]) => Promise<PollSummary | null>;
}) {
  const poll = status.poll;
  const [selected, setSelected] = React.useState<Set<number>>(
    () => new Set(poll?.ownVotes ?? []),
  );
  const [showResults, setShowResults] = React.useState(() =>
    Boolean(poll?.voted || poll?.expired),
  );
  const [pending, setPending] = React.useState(false);

  React.useEffect(() => {
    setSelected(new Set(poll?.ownVotes ?? []));
    setShowResults(Boolean(poll?.voted || poll?.expired));
  }, [poll?.id, poll?.voted, poll?.expired, poll?.ownVotes]);

  if (!poll || poll.options.length === 0) return null;

  const canVote = votingSupported && poll.voted !== true && !poll.expired;
  const totalVotes = Math.max(0, poll.votesCount);
  const selectedCount = selected.size;

  const toggleOption = (index: number) => {
    if (!canVote || pending) return;
    setSelected((current) => {
      if (!poll.multiple) return new Set([index]);
      const next = new Set(current);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  };

  const submitVote = async () => {
    if (!canVote || selectedCount === 0 || pending) return;
    setPending(true);
    const updated = await onVote([...selected].sort((a, b) => a - b));
    setPending(false);
    if (updated) setShowResults(true);
  };

  return (
    <div className="mt-3 space-y-2 text-sm">
      <div className="space-y-1.5">
        {poll.options.map((option, index) => (
          <PollOptionRow
            key={`${poll.id}-${index}`}
            option={option}
            poll={poll}
            checked={selected.has(index)}
            disabled={!canVote || pending}
            showResults={showResults || poll.voted === true || poll.expired}
            totalVotes={totalVotes}
            onToggle={() => toggleOption(index)}
          />
        ))}
      </div>
      <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs text-overlay0">
        {canVote ? (
          <button
            type="button"
            className="btn btn-outline btn-xs min-h-7 border-surface1 px-3"
            disabled={selectedCount === 0 || pending}
            onClick={() => void submitVote()}
          >
            {pending ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
            {t("Vote")}
          </button>
        ) : null}
        {!showResults && !poll.voted && (
          <button
            type="button"
            className="text-subtext0 hover:text-blue"
            onClick={() => setShowResults(true)}
          >
            {t("Show results")}
          </button>
        )}
        <span>{formatPollCount(poll)}</span>
        <span>{formatPollExpiry(poll)}</span>
      </div>
    </div>
  );
}

function PollOptionRow({
  option,
  poll,
  checked,
  disabled,
  showResults,
  totalVotes,
  onToggle,
}: {
  option: PollSummary["options"][number];
  poll: PollSummary;
  checked: boolean;
  disabled: boolean;
  showResults: boolean;
  totalVotes: number;
  onToggle: () => void;
}) {
  const votes = option.votesCount ?? 0;
  const percentage =
    totalVotes > 0 ? Math.round((votes / totalVotes) * 100) : 0;
  const inputType = poll.multiple ? "checkbox" : "radio";

  return (
    <label
      className={`block rounded border border-surface0 bg-base-200/50 px-2.5 py-2 ${disabled ? "" : "cursor-pointer hover:border-blue/70"}`}
    >
      <span className="flex min-w-0 items-center gap-2">
        <input
          type={inputType}
          name={`poll-${poll.id}`}
          className={`${poll.multiple ? "checkbox checkbox-xs" : "radio radio-xs"} border-overlay0 bg-base`}
          checked={checked}
          disabled={disabled}
          onChange={onToggle}
        />
        <span className="min-w-0 flex-1 text-text">
          <CustomEmojiText text={option.title} emojis={poll.emojis} />
        </span>
        {showResults ? (
          <span className="shrink-0 tabular-nums text-xs text-subtext0">
            {percentage}%
          </span>
        ) : null}
      </span>
      {showResults ? (
        <span className="mt-1.5 block h-1.5 overflow-hidden rounded bg-surface1">
          <span
            className="block h-full rounded bg-blue"
            style={{ width: `${percentage}%` }}
          />
        </span>
      ) : null}
    </label>
  );
}

function formatPollCount(poll: PollSummary) {
  const count = poll.votersCount ?? poll.votesCount;
  return t("{count} voters", { count: formatNumber(count) });
}

function formatPollExpiry(poll: PollSummary) {
  if (poll.expired) return t("Closed");
  if (!poll.expiresAt) return t("No deadline");

  const remainingMs = Date.parse(poll.expiresAt) - Date.now();
  if (!Number.isFinite(remainingMs) || remainingMs <= 0)
    return t("Closing soon");
  const minutes = Math.ceil(remainingMs / 60_000);
  if (minutes < 60) return t("{count}m left", { count: minutes });
  const hours = Math.ceil(minutes / 60);
  if (hours < 48) return t("{count}h left", { count: hours });
  return t("{count}d left", { count: Math.ceil(hours / 24) });
}
