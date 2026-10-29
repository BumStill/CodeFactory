# SPDX-License-Identifier: Apache-2.0
"""Parse, NEVER execute, a restricted Python file-edit program.

Unknown syntax fails closed. Only literals, string transformations and file
handles opened on statically known workspace paths are capabilities here.
"""
import ast
import pathlib
import sys

root = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 and sys.argv[1] else None
STR, INT, BOOL, FILE, NONE = 'str', 'int', 'bool', 'file', 'none'
values = {}
functions = {}


def path(value):
    if not isinstance(value, str) or not value or '\\' in value:
        raise ValueError('dynamic or nonportable path')
    p = pathlib.PurePosixPath(value)
    if p.is_absolute() or ':' in value or any(x in ('..', '.', '~') for x in p.parts):
        raise ValueError('outside path')
    for part in p.parts:
        if part.startswith(('$', '~')) or part in ('.git', '.ssh', '.aws', '.gnupg', '.netrc', 'credentials') or part.startswith('.env') or part.endswith(('.pem', '.key')) or part in ('id_rsa', 'id_ed25519'):
            raise ValueError('protected path')
    if root:
        resolved = (root / value).resolve()
        if resolved == root or root not in resolved.parents:
            raise ValueError('symlink escape')
    return value


def expr(n, env):
    if isinstance(n, ast.Constant):
        if type(n.value) is str: return (STR, n.value)
        if type(n.value) is int: return (INT, n.value)
        if type(n.value) is bool: return (BOOL, n.value)
        if n.value is None: return (NONE, None)
    if isinstance(n, ast.Name) and n.id in env:
        return env[n.id]
    if isinstance(n, ast.BinOp) and isinstance(n.op, ast.Add):
        a, b = expr(n.left, env), expr(n.right, env)
        if a[0] == b[0] == STR:
            return (STR, a[1] + b[1] if a[1] is not None and b[1] is not None else None)
    if isinstance(n, ast.Subscript):
        a = expr(n.value, env)
        if a[0] != STR: raise ValueError('non-string slice')
        if isinstance(n.slice, ast.Slice):
            for x in (n.slice.lower, n.slice.upper, n.slice.step):
                if x is not None and expr(x, env)[0] != INT: raise ValueError('dynamic slice')
        elif expr(n.slice, env)[0] != INT: raise ValueError('dynamic index')
        return (STR, None)
    if isinstance(n, ast.Compare):
        args = [expr(n.left, env)] + [expr(x, env) for x in n.comparators]
        if any(t[0] not in (STR, INT, BOOL, NONE) for t in args): raise ValueError('comparison capability')
        if not all(isinstance(x, (ast.Eq, ast.NotEq, ast.Gt, ast.GtE, ast.Lt, ast.LtE)) for x in n.ops): raise ValueError('comparison')
        return (BOOL, None)
    if isinstance(n, ast.Call) and not n.keywords:
        args = [expr(x, env) for x in n.args]
        if isinstance(n.func, ast.Name):
            if n.func.id == 'open' and 1 <= len(args) <= 2:
                if args[0][0] != STR: raise ValueError('path type')
                path(args[0][1])
                mode = args[1][1] if len(args) == 2 else 'r'
                if mode not in ('r', 'w', 'a', 'rt', 'wt', 'at'): raise ValueError('mode')
                return (FILE, mode)
            if n.func.id in functions:
                fn = functions[n.func.id]
                names = [a.arg for a in fn.args.args]
                defaults = [expr(x, env) for x in fn.args.defaults]
                if not len(names) - len(defaults) <= len(args) <= len(names): raise ValueError('arity')
                actual = args + defaults[len(defaults) - (len(names) - len(args)): ] if len(args) < len(names) else args
                local = dict(env)
                local.update(zip(names, actual))
                # Recursion cannot grant an unchecked capability.
                saved = functions.pop(n.func.id)
                try: statements(fn.body, local, in_function=True)
                finally: functions[n.func.id] = saved
                for statement in fn.body:
                    if isinstance(statement, ast.Global):
                        for name in statement.names: env[name] = local[name]
                return (NONE, None)
        if isinstance(n.func, ast.Attribute):
            receiver = expr(n.func.value, env)
            method = n.func.attr
            if receiver[0] == FILE:
                if method == 'read' and not args and receiver[1] in ('r', 'rt'): return (STR, None)
                if method == 'write' and len(args) == 1 and args[0][0] == STR and receiver[1] in ('w', 'wt', 'a', 'at'): return (INT, None)
            if receiver[0] == STR:
                if method == 'replace' and len(args) in (2, 3) and all(x[0] == STR for x in args[:2]) and (len(args) == 2 or args[2][0] == INT): return (STR, None)
                if method == 'count' and len(args) == 1 and args[0][0] == STR: return (INT, None)
                if method in ('strip', 'lstrip', 'rstrip') and len(args) <= 1 and all(x[0] == STR for x in args): return (STR, None)
    raise ValueError('unsupported expression: ' + type(n).__name__)


def statements(body, env, in_function=False):
    for n in body:
        if isinstance(n, ast.Assign) and all(isinstance(x, ast.Name) for x in n.targets):
            value = expr(n.value, env)
            for name in n.targets:
                if name.id.startswith('__') or name.id in ('open',): raise ValueError('builtin replacement')
                env[name.id] = value
        elif isinstance(n, ast.Expr):
            expr(n.value, env)
        elif isinstance(n, ast.Assert):
            if expr(n.test, env)[0] != BOOL: raise ValueError('assertion')
            if n.msg is not None and expr(n.msg, env)[0] != STR: raise ValueError('assert message')
        elif isinstance(n, ast.Global) and in_function:
            if any(x not in env for x in n.names): raise ValueError('unknown global')
        elif isinstance(n, ast.FunctionDef) and not in_function:
            if n.decorator_list or n.returns or n.args.vararg or n.args.kwarg or n.args.kwonlyargs or n.args.posonlyargs or any(x.annotation for x in n.args.args): raise ValueError('function effects')
            if n.name in env or n.name == 'open': raise ValueError('shadowed name')
            functions[n.name] = n
        else:
            raise ValueError('unsupported statement: ' + type(n).__name__)

def node_statements(source):
    # Parse a deliberately small JavaScript grammar without evaluating JS.
    # Python's tokenizer is not used: JS quote escapes and statement delimiters
    # are consumed explicitly, and template strings / computed calls fail shut.
    import re
    token = re.compile(r"\s*(?:(?P<string>'(?:[^'\\\n]|\\['\\])*'|\"(?:[^\"\\\n]|\\[\"\\])*\")|(?P<name>[A-Za-z_][A-Za-z_0-9]*)|(?P<symbol>[=().,;]))")
    tokens = []
    offset = 0
    while source[offset:].strip():
        match = token.match(source, offset)
        if not match: raise ValueError('unsupported JS token')
        tokens.append((match.lastgroup, match.group(match.lastgroup)))
        offset = match.end()
    index = 0
    env = {}

    def take(value=None):
        nonlocal index
        if index >= len(tokens): raise ValueError('incomplete JS')
        kind, raw = tokens[index]
        if value is not None and raw != value: raise ValueError('JS syntax')
        index += 1
        return kind, raw

    def expression():
        nonlocal index
        kind, raw = take()
        if kind == 'string':
            result = (STR, ast.literal_eval(raw))
        elif raw == 'require':
            take('(')
            kind, module = take()
            if kind != 'string' or ast.literal_eval(module) not in ('fs', 'node:fs'): raise ValueError('JS capability')
            take(')')
            result = ('fs', None)
        elif kind == 'name' and raw in env:
            result = env[raw]
        else: raise ValueError('JS unknown name')
        while index < len(tokens) and tokens[index][1] == '.':
            take('.')
            _, method = take()
            take('(')
            args = []
            if index < len(tokens) and tokens[index][1] != ')':
                args.append(expression())
                while index < len(tokens) and tokens[index][1] == ',':
                    take(',')
                    args.append(expression())
            take(')')
            if result[0] == 'fs' and method == 'readFileSync' and len(args) == 2 and args[1] == (STR, 'utf8'):
                path(args[0][1])
                result = (STR, None)
            elif result[0] == 'fs' and method == 'writeFileSync' and len(args) == 2 and all(x[0] == STR for x in args):
                path(args[0][1])
                result = (NONE, None)
            elif result[0] == STR and method in ('replace', 'replaceAll') and len(args) == 2 and all(x[0] == STR for x in args):
                result = (STR, None)
            else: raise ValueError('JS external or unknown operation')
        return result

    while index < len(tokens):
        declaration = tokens[index][1] in ('const', 'let', 'var')
        if declaration: take()
        if index + 1 < len(tokens) and tokens[index][0] == 'name' and tokens[index + 1][1] == '=':
            _, name = take()
            take('=')
            env[name] = expression()
        elif declaration: raise ValueError('JS declaration')
        else: expression()
        take(';')


try:
    source = sys.stdin.read()
    if len(sys.argv) > 2 and sys.argv[2] == 'node':
        node_statements(source)
        sys.exit(0)
    tree = ast.parse(source)
    statements(tree.body, values)
    # Even uncalled functions must not contain unchecked effects.
    for name, fn in list(functions.items()):
        local = dict(values)
        local.update((a.arg, (STR, None)) for a in fn.args.args)
        defaults = [expr(x, values) for x in fn.args.defaults]
        for a, default in zip(fn.args.args[len(fn.args.args)-len(defaults):], defaults): local[a.arg] = default
        saved = functions.pop(name)
        try: statements(fn.body, local, in_function=True)
        finally: functions[name] = saved
except (ValueError, SyntaxError, TypeError, RecursionError, OSError, KeyError):
    sys.exit(1)
