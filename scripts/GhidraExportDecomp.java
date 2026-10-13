// Ghidra headless postScript：平行反編譯全部函式，依位址順序寫成單一可 grep 的 C 檔。
// 由 scripts/df_snapshot.py `decompile` 呼叫：... -postScript GhidraExportDecomp.java <out.c>
// 每個函式前一行是 "// ============ <name> @ 0x<va> ============"，位址格式同 exe/labels.tsv。
// @category DF

import ghidra.app.decompiler.DecompileOptions;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.decompiler.parallel.ChunkingParallelDecompiler;
import ghidra.app.decompiler.parallel.DecompilerCallback;
import ghidra.app.decompiler.parallel.ParallelDecompiler;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.util.task.TaskMonitor;

import java.io.BufferedWriter;
import java.io.FileOutputStream;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.util.AbstractMap;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

public class GhidraExportDecomp extends GhidraScript {
    private static final int CHUNK = 2000;
    private static final int TIMEOUT_SECONDS = 120;

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 1) {
            throw new IllegalArgumentException("usage: GhidraExportDecomp.java <output.c>");
        }
        List<Function> functions = new ArrayList<>();
        for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
            if (!f.isThunk() && !f.isExternal()) {
                functions.add(f);
            }
        }
        DecompilerCallback<Map.Entry<Address, String>> callback =
            new DecompilerCallback<>(currentProgram, d -> {
                d.setOptions(new DecompileOptions());
                d.toggleSyntaxTree(false);
            }) {
                @Override
                public Map.Entry<Address, String> process(DecompileResults r, TaskMonitor m) {
                    Function f = r.getFunction();
                    String head = "// ============ " + f.getName(true) + " @ 0x"
                        + Long.toHexString(f.getEntryPoint().getOffset()) + " ============\n";
                    String body = r.decompileCompleted()
                        ? r.getDecompiledFunction().getC() + "\n"
                        : "// DECOMPILE FAILED: " + String.valueOf(r.getErrorMessage()).strip() + "\n\n";
                    return new AbstractMap.SimpleEntry<>(f.getEntryPoint(), head + body);
                }
            };
        callback.setTimeout(TIMEOUT_SECONDS);
        ChunkingParallelDecompiler<Map.Entry<Address, String>> decompiler =
            ParallelDecompiler.createChunkingParallelDecompiler(callback, monitor);
        int failed = 0;
        try (Writer out = new BufferedWriter(
                new OutputStreamWriter(new FileOutputStream(args[0]), StandardCharsets.UTF_8), 1 << 20)) {
            out.write("// Decompiled by Ghidra (headless) from " + currentProgram.getName() + "\n");
            out.write("// image base " + currentProgram.getImageBase() + ", " + functions.size() + " functions\n\n");
            for (int i = 0; i < functions.size(); i += CHUNK) {
                if (monitor.isCancelled()) {
                    break;
                }
                List<Map.Entry<Address, String>> results =
                    new ArrayList<>(decompiler.decompileFunctions(functions.subList(i, Math.min(i + CHUNK, functions.size()))));
                results.sort(Map.Entry.comparingByKey());
                for (Map.Entry<Address, String> e : results) {
                    if (e.getValue().contains("// DECOMPILE FAILED: ")) {
                        failed++;
                    }
                    out.write(e.getValue());
                }
                println("decomp " + Math.min(i + CHUNK, functions.size()) + "/" + functions.size());
            }
        } finally {
            decompiler.dispose();
            callback.dispose();
        }
        println("export done: " + functions.size() + " functions, " + failed + " failed -> " + args[0]);
    }
}
