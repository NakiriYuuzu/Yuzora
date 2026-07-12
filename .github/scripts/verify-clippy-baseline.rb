#!/usr/bin/env ruby
# frozen_string_literal: true

require "json"
require "open3"

baseline_path = ARGV.fetch(0)
command = %w[cargo clippy --quiet --locked --all-targets --message-format=json]
observed = Hash.new(0)
compiler_errors = []

status = nil
Open3.popen3(*command) do |stdin, stdout, stderr, wait_thread|
  stdin.close
  stderr_reader = Thread.new do
    stderr.each_line { |line| warn line }
  end

  stdout.each_line do |line|
    row = JSON.parse(line)
    next unless row["reason"] == "compiler-message"

    message = row["message"]
    if message["level"] == "error"
      compiler_errors << (message["rendered"] || message["message"])
      next
    end
    next unless message["level"] == "warning"

    span = message.fetch("spans", []).find { |candidate| candidate["is_primary"] } || {}
    fingerprint = [
      message.dig("code", "code") || "warning",
      span["file_name"] || "",
      span["line_start"] || 0,
      message["message"],
    ]
    observed[fingerprint] += 1
  rescue JSON::ParserError
    warn "non-JSON cargo output: #{line}"
  end

  status = wait_thread.value
  stderr_reader.join
end

unless status.success?
  compiler_errors.each { |error| warn error }
  exit status.exitstatus || 1
end

expected = JSON.parse(File.read(baseline_path)).each_with_object({}) do |row, result|
  fingerprint = [row.fetch("code"), row.fetch("file"), row.fetch("line"), row.fetch("message")]
  result[fingerprint] = row.fetch("count")
end

if observed != expected
  missing = expected.to_a - observed.to_a
  unexpected = observed.to_a - expected.to_a
  warn "Clippy warning baseline mismatch."
  missing.each { |fingerprint, count| warn "missing #{count}x #{fingerprint.join(" | ")}" }
  unexpected.each { |fingerprint, count| warn "unexpected #{count}x #{fingerprint.join(" | ")}" }
  exit 1
end

puts "Clippy warning baseline matched #{observed.values.sum} diagnostics."
