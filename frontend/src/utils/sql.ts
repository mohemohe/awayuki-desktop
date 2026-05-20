function isSqlIdentifierChar(char: string | undefined) {
  return Boolean(char && /[A-Za-z0-9_$]/.test(char));
}

export function hasTopLevelSqlLimit(sql: string) {
  let depth = 0;

  for (let index = 0; index < sql.length; index += 1) {
    const char = sql[index];
    const next = sql[index + 1];

    if (char === "-" && next === "-") {
      index += 2;
      while (index < sql.length && sql[index] !== "\n") index += 1;
      continue;
    }

    if (char === "/" && next === "*") {
      index += 2;
      while (
        index < sql.length - 1 &&
        !(sql[index] === "*" && sql[index + 1] === "/")
      ) {
        index += 1;
      }
      index += 1;
      continue;
    }

    if (char === "'" || char === '"' || char === "`") {
      const quote = char;
      index += 1;
      while (index < sql.length) {
        if (sql[index] === quote) {
          if (sql[index + 1] === quote) {
            index += 2;
            continue;
          }
          break;
        }
        index += 1;
      }
      continue;
    }

    if (char === "[") {
      while (index < sql.length && sql[index] !== "]") index += 1;
      continue;
    }

    if (char === "(") {
      depth += 1;
      continue;
    }
    if (char === ")") {
      depth = Math.max(0, depth - 1);
      continue;
    }

    if (
      depth === 0 &&
      sql.slice(index, index + 5).toLowerCase() === "limit" &&
      !isSqlIdentifierChar(sql[index - 1]) &&
      !isSqlIdentifierChar(sql[index + 5])
    ) {
      return true;
    }
  }

  return false;
}
