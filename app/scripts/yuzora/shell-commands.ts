// yuzora(release): the command names a bash script runs, for the contract test of
// .github/workflows/app-release.yml (app-release.test.ts): the jobs that write to a
// release may run only a short list of programs, whatever file they hold.
//
// Enough bash for that workflow's scripts: pipes, lists, if/while/for, functions, $( )
// and backticks, quotes, here-documents and redirections. No arithmetic, no case, no
// process substitution. What it does not model reads as ordinary words, so a script that
// leans on it shows up in the list instead of hiding from it.
const STAYS = new Set(["if", "then", "elif", "else", "while", "until", "do", "!", "{", "time"]);
const ENDS = new Set(["fi", "done", "}", "esac", "in", "for", "case", "function", "select"]);

/// The first word of every simple command in `script`, in order, with those of every
/// here-document body (read as a script too) after the line that opens it.
export function commandWords(script: string): string[] {
  const out: string[] = [];
  const waiting: { tag: string }[] = [];
  let i = 0;

  const skipBodies = () => {
    while (waiting.length > 0) {
      const { tag } = waiting.shift()!;
      const from = i;
      let to = i;
      while (i < script.length) {
        const nl = script.indexOf("\n", i);
        const line = script.slice(i, nl < 0 ? undefined : nl);
        i = nl < 0 ? script.length : nl + 1;
        if (line.trim() === tag) break;
        to = i;
      }
      out.push(...commandWords(script.slice(from, to)));
    }
  };

  // Past a `${ ... }`, to the closing brace.
  const braces = () => {
    let depth = 0;
    const start = i;
    while (i < script.length) {
      if (script[i] === "{") depth++;
      if (script[i] === "}" && --depth === 0) return script.slice(start, ++i);
      i++;
    }
    return script.slice(start);
  };

  const backticks = () => {
    const end = script.indexOf("`", i + 1);
    const inner = script.slice(i + 1, end < 0 ? undefined : end);
    i = end < 0 ? script.length : end + 1;
    out.push(...commandWords(inner));
  };

  const doubleQuoted = () => {
    let text = '"';
    i++;
    while (i < script.length && script[i] !== '"') {
      const c = script[i];
      if (c === "\\") {
        text += script.slice(i, i + 2);
        i += 2;
      } else if (c === "$" && script[i + 1] === "(") {
        i += 2;
        list(")");
        text += "$()";
      } else if (c === "$" && script[i + 1] === "{") {
        i++;
        text += "$" + braces();
      } else if (c === "`") {
        backticks();
        text += "``";
      } else {
        text += c;
        i++;
      }
    }
    i++;
    return text + '"';
  };

  // A list of commands up to the `)` that closes a $( ), or to the end of the script.
  function list(close: string | null) {
    let expect = true;
    let word = "";
    let has = false;
    let target = false; // the next word is the target of a redirection, not a command
    const end = () => {
      if (!has) return;
      const w = word;
      word = "";
      has = false;
      if (target) {
        target = false;
        return;
      }
      if (!expect) return;
      if (/^[A-Za-z_][A-Za-z0-9_]*(\[[^\]]*\])?\+?=/.test(w)) return;
      if (STAYS.has(w)) return;
      if (ENDS.has(w)) {
        expect = false;
        return;
      }
      out.push(w);
      expect = false;
    };
    const next = () => {
      end();
      expect = true;
    };

    while (i < script.length) {
      const c = script[i];
      if (c === "\\") {
        if (script[i + 1] !== "\n") {
          word += script[i + 1] ?? "";
          has = true;
        }
        i += 2;
      } else if (c === "'") {
        const close = script.indexOf("'", i + 1);
        word += script.slice(i, close < 0 ? undefined : close + 1);
        i = close < 0 ? script.length : close + 1;
        has = true;
      } else if (c === '"') {
        word += doubleQuoted();
        has = true;
      } else if (c === "$" && script[i + 1] === "(") {
        i += 2;
        list(")");
        word += "$()";
        has = true;
      } else if (c === "$" && script[i + 1] === "{") {
        i++;
        word += "$" + braces();
        has = true;
      } else if (c === "`") {
        backticks();
        word += "``";
        has = true;
      } else if (c === "#" && !has) {
        while (i < script.length && script[i] !== "\n") i++;
      } else if (c === "\n") {
        next();
        i++;
        skipBodies();
      } else if (c === " " || c === "\t") {
        end();
        i++;
      } else if (c === "(" && has && /\+?=$/.test(word)) {
        // name=(a b "c"): an array's elements are words, not commands.
        i++;
        while (i < script.length && script[i] !== ")") {
          if (script[i] === "'") i = script.indexOf("'", i + 1) + 1 || script.length;
          else if (script[i] === '"') doubleQuoted();
          else i++;
        }
        i++;
        word += "()";
      } else if (c === ";" || c === "|" || c === "(") {
        next();
        i++;
      } else if (c === "&") {
        next();
        i++;
      } else if (c === ")") {
        i++;
        if (close === ")") {
          end();
          return;
        }
        next(); // `name()` of a function: its body is what comes next
      } else if (c === "<" || c === ">") {
        if (/^[0-9]+$/.test(word)) {
          word = "";
          has = false;
        } else {
          end();
        }
        const heredoc = script.startsWith("<<", i) && !script.startsWith("<<<", i);
        while (script[i] === "<" || script[i] === ">") i++;
        if (script[i] === "-" && heredoc) i++;
        if (script[i] === "&") {
          i++;
          if (/[0-9-]/.test(script[i] ?? "")) {
            while (/[0-9-]/.test(script[i] ?? "")) i++;
            continue;
          }
        }
        if (heredoc) {
          while (script[i] === " " || script[i] === "\t") i++;
          const m = /^(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1/.exec(script.slice(i));
          if (m) {
            waiting.push({ tag: m[2] });
            i += m[0].length;
          }
        } else {
          target = true;
        }
      } else {
        word += c;
        has = true;
        i++;
      }
    }
    end();
  }

  list(null);
  return out;
}
