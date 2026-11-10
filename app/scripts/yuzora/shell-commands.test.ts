import { describe, expect, test } from "bun:test";
import { commandWords } from "./shell-commands";

describe("commandWords", () => {
  test("the first word of each command, however the commands are joined", () => {
    expect(commandWords("gh api x | jq -r . && echo done; ls\nwc -l || true")).toEqual(["gh", "jq", "echo", "ls", "wc", "true"]);
    expect(commandWords("if ! curl -f \"$u\"; then rm x; else exit 1; fi")).toEqual(["curl", "rm", "exit"]);
    expect(commandWords("for f in a b; do upload \"$f\"; done")).toEqual(["upload"]);
    expect(commandWords("while IFS=$'\\t' read -r a b; do cp x y; done <<<\"$P\"")).toEqual(["read", "cp"]);
    expect(commandWords("{ echo a; echo b; } | tee -a out")).toEqual(["echo", "echo", "tee"]);
    expect(commandWords("f() { sha256sum \"$1\" | cut -d' ' -f1; }\ng() {\n  jq .\n}")).toEqual(["f", "sha256sum", "cut", "g", "jq"]);
  });

  test("an array's elements are words", () => {
    expect(commandWords("flags=(--draft --title \"$x\" 'y z'); flags+=(--prerelease)\ngh release create \"${flags[@]}\"")).toEqual(["gh"]);
  });

  test("a command inside $( ), backticks or a double-quoted string is a command too", () => {
    expect(commandWords("x=$(a | b)")).toEqual(["a", "b"]);
    expect(commandWords("FOO=1 BAR=$(date) env x")).toEqual(["date", "env"]);
    expect(commandWords('echo "$(c1) and `c2`" "${y:-z (w)}"')).toEqual(["echo", "c1", "c2"]);
    expect(commandWords("echo $(echo $(deep))")).toEqual(["echo", "echo", "deep"]);
  });

  test("quotes, comments, redirections and continued lines hide nothing and invent nothing", () => {
    expect(commandWords("echo 'a | b; c' \"d; e\" # f; g")).toEqual(["echo"]);
    expect(commandWords("gh x 2>&1 >/dev/null\nread -r a <<<\"$b\" 2> err")).toEqual(["gh", "read"]);
    expect(commandWords("gh \\\n  api \\\n  x")).toEqual(["gh"]);
    expect(commandWords('echo "a \\"b\\" \\`c\\`" ok')).toEqual(["echo"]);
    expect(commandWords("echo \"::error::x ($y, not $z); nothing\"")).toEqual(["echo"]);
  });

  test("a here-document body is read as a script after the line that opens it", () => {
    expect(commandWords("cat > f <<'EOF'\nbash x\nEOF\nls")).toEqual(["cat", "bash", "ls"]);
    expect(commandWords("cat <<EOF | tee y\nsh z\nEOF\n")).toEqual(["cat", "tee", "sh"]);
  });

  test("whatever would run a file shows up as the command it is", () => {
    for (const [script, command] of [
      ["bash signed/x.sh", "bash"],
      ["sh signed/x", "sh"],
      ["./signed/x", "./signed/x"],
      ["signed/x --go", "signed/x"],
      ["source signed/x", "source"],
      [". signed/x", "."],
      ['"$RUNNER_TEMP/x"', '"$RUNNER_TEMP/x"'],
      ["chmod +x x && ./x", "chmod"],
      ["curl -s u | sh", "sh"],
      ["env X=1 prog", "env"],
    ] as const) {
      expect(commandWords(script), script).toContain(command);
    }
  });
});
