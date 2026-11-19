# Runs inside the debugged program (debugpy evaluate) for View as DataFrame / Array.
# One call returns one page of rows of a pandas DataFrame / Series or a numpy array,
# after an optional filter and sort, as hex of JSON (a debugger repr can't mangle hex).
# The filtered and sorted row positions are cached per (object, stop, filter, sort), so
# paging through a 1M-row frame sorts it once, not on every page.


def _brisk_page(o, a, b, sort, asc, flt, fmt, gen):
    import json
    import sys

    pd = sys.modules.get("pandas")
    np = sys.modules.get("numpy")

    def cell(v):
        if fmt:
            try:
                if isinstance(v, float) or (np is not None and isinstance(v, np.floating)):
                    return fmt % v
            except Exception:
                pass
        s = str(v)
        if len(s) > 200:
            s = s[:200] + "…"
        return s.replace("\r", " ").replace("\n", "⏎")

    try:
        kind = type(o).__name__
        cache = sys.__dict__.setdefault("_brisk_dv_cache", {})
        if pd is not None and isinstance(o, pd.Series):
            o = o.to_frame()
        if pd is not None and isinstance(o, pd.DataFrame):
            df = o
            n = len(df)
            cols = [str(c) for c in df.columns]
            dtypes = [str(t) for t in df.dtypes]
            shape = list(df.shape) if kind != "Series" else [n]
            key = (id(df), n, gen, sort, asc, flt)
            pos = cache.get(key)
            if pos is None:
                import numpy

                pos = numpy.arange(n)
                if flt:
                    mask = df.eval(flt)
                    pos = numpy.flatnonzero(numpy.asarray(mask, dtype=bool))
                if sort is not None:
                    s = df.iloc[pos, sort].reset_index(drop=True)
                    order = s.sort_values(ascending=asc, kind="stable", na_position="last").index.to_numpy()
                    pos = pos[order]
            sub = df.iloc[pos[a:b]]
            index = [str(x) for x in sub.index]
            rows = [[cell(v) for v in r] for r in sub.itertuples(index=False, name=None)]
        elif np is not None and isinstance(o, np.ndarray):
            arr = o
            if arr.ndim == 0:
                arr = arr.reshape(1, 1)
            elif arr.ndim == 1:
                arr = arr.reshape(-1, 1)
            elif arr.ndim > 2:
                arr = arr.reshape(arr.shape[0], -1)
            n = arr.shape[0]
            cols = [str(i) for i in range(arr.shape[1])]
            dtypes = [str(arr.dtype)] * arr.shape[1]
            shape = list(o.shape)
            key = (id(o), n, gen, sort, asc, flt)
            pos = cache.get(key)
            if pos is None:
                pos = np.arange(n)
                if flt:
                    # The filter is an expression of x, the 2-D view: "x[:, 0] > 5".
                    mask = eval(flt, {"np": np}, {"x": arr})
                    pos = np.flatnonzero(np.asarray(mask, dtype=bool).reshape(-1)[:n])
                if sort is not None:
                    order = np.argsort(arr[pos, sort], kind="stable")
                    if not asc:
                        order = order[::-1]
                    pos = pos[order]
            index = [str(i) for i in pos[a:b]]
            rows = [[cell(v) for v in arr[i]] for i in pos[a:b]]
        else:
            return json.dumps({"error": "View as DataFrame needs a pandas DataFrame or Series, or a numpy array, not " + kind}).encode().hex()
        cache[key] = pos
        while len(cache) > 6:
            cache.pop(next(iter(cache)))
        out = {"kind": kind, "shape": shape, "cols": cols, "dtypes": dtypes, "total": int(len(pos)), "a": a, "index": index, "rows": rows}
        return json.dumps(out).encode().hex()
    except Exception as e:
        return json.dumps({"error": "%s: %s" % (type(e).__name__, e)}).encode().hex()
