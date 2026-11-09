"""A soma conserva a ordem do arquivo sem depender da descoberta ou dos commits do índice."""


import pytest


@pytest.mark.parametrize("initial_order", [("a", "b"), ("b", "a")])
def test_existing_index_reads_by_path_after_insertion_and_update(tmp_path, monkeypatch, initial_order):
    from app import costs, costs_cache  # noqa: F401

    monkeypatch.setattr(costs_cache, "_CACHE_DIR", tmp_path / "index")
    connection = costs_cache._abrir()
    try:
        for name in initial_order:
            connection.execute("INSERT INTO files(path,scope,versao) VALUES(?, 'scope', 'old')", (f"/synthetic/{name}.jsonl",))
        ids = dict(connection.execute("SELECT substr(path,12,1), id FROM files"))

        def insert(name):
            values = [12, 11] if name == "a" else [22, 21]
            for value in values:
                connection.execute("INSERT INTO custo(file_id,input) VALUES(?,?)", (ids[name], value))
                connection.execute("INSERT INTO uso(file_id,input) VALUES(?,?)", (ids[name], value))

        for name in initial_order:
            insert(name)

        def assert_order():
            column = costs_cache.CAMPOS_CUSTO.index("input")
            assert [row[column] for row in costs_cache.ler_custos("scope")] == [12, 11, 22, 21]
            column = costs_cache.CAMPOS_USO.index("input")
            assert [row[column] for row in costs_cache.iter_usage_rows("scope", "account")] == [12, 11, 22, 21]

        assert_order()
        for table in ("custo", "uso"):
            connection.execute(f"DELETE FROM {table} WHERE file_id=?", (ids["a"],))
        insert("a")
        assert_order()
    finally:
        connection.close()
