package dev.horizonlab.neoforge;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.stream.JsonWriter;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.Container;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.BlockEntityTicker;
import net.minecraft.world.level.block.entity.BlockEntityType;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;

/**
 * Dev-only machine oracle for Horizon's code-mod harness (tools/mod_machine_harness.py): places
 * each scenario's block, fills its container's slots, runs its block entity ticker N times and
 * writes the slots, the asked block-state properties and the asked integer fields of its saved
 * tag after every tick. The same scenarios run on the Horizon port through
 * crates/horizon-client/examples/mod_machine_probe.rs. Never shipped.
 */
final class MachineTrace {
    private static final BlockPos AT = new BlockPos(0, 100, 0);

    private MachineTrace() {}

    static void run(MinecraftServer server, Path scenarios, JsonWriter json) throws IOException {
        ServerLevel level = server.overworld();
        JsonArray list = JsonParser.parseString(Files.readString(scenarios)).getAsJsonArray();
        json.beginArray();
        for (JsonElement e : list) {
            scenario(server, level, e.getAsJsonObject(), json);
        }
        json.endArray();
    }

    private static ItemStack stack(JsonElement v) {
        if (v == null || v.isJsonNull()) {
            return ItemStack.EMPTY;
        }
        JsonArray a = v.getAsJsonArray();
        return new ItemStack(BuiltInRegistries.ITEM.get(ResourceLocation.parse(a.get(0).getAsString())),
                a.get(1).getAsInt());
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static BlockState with(BlockState state, String name, String value) {
        for (Property<?> p : state.getProperties()) {
            if (p.getName().equals(name)) {
                Property raw = p;
                java.util.Optional<Comparable> parsed = raw.getValue(value);
                return parsed.isPresent() ? (BlockState) state.setValue(raw, parsed.get()) : state;
            }
        }
        return state;
    }

    private static String property(BlockState state, String name) {
        for (Property<?> p : state.getProperties()) {
            if (p.getName().equals(name)) {
                return String.valueOf(state.getValue(p));
            }
        }
        return null;
    }

    @SuppressWarnings("unchecked")
    private static void scenario(MinecraftServer server, ServerLevel level, JsonObject s, JsonWriter json)
            throws IOException {
        Block block = BuiltInRegistries.BLOCK.get(ResourceLocation.parse(s.get("block").getAsString()));
        BlockState state = block.defaultBlockState();
        JsonObject properties = s.has("properties") ? s.getAsJsonObject("properties") : new JsonObject();
        for (Map.Entry<String, JsonElement> p : properties.entrySet()) {
            state = with(state, p.getKey(), p.getValue().getAsJsonArray().get(0).getAsString());
        }
        level.setBlock(AT, state, 3);
        BlockEntity be = level.getBlockEntity(AT);
        if (!(be instanceof Container container)) {
            throw new IllegalStateException(s.get("block") + " has no container block entity");
        }
        if (s.has("slots")) {
            for (Map.Entry<String, JsonElement> slot : s.getAsJsonObject("slots").entrySet()) {
                container.setItem(Integer.parseInt(slot.getKey()), stack(slot.getValue()));
            }
        }
        JsonArray fields = s.has("fields") ? s.getAsJsonArray("fields") : new JsonArray();
        JsonArray events = s.has("events") ? s.getAsJsonArray("events") : new JsonArray();
        json.beginObject();
        json.name("name").value(s.get("name").getAsString());
        json.name("trace").beginArray();
        int ticks = s.get("ticks").getAsInt();
        for (int tick = 0; tick < ticks; tick++) {
            for (JsonElement e : events) {
                JsonObject ev = e.getAsJsonObject();
                if (ev.get("tick").getAsInt() == tick) {
                    container.setItem(ev.get("slot").getAsInt(), stack(ev.get("stack")));
                }
            }
            BlockState now = level.getBlockState(AT);
            BlockEntityTicker<BlockEntity> ticker =
                    now.getTicker(level, (BlockEntityType<BlockEntity>) be.getType());
            if (ticker != null) {
                ticker.tick(level, AT, now, be);
            }
            now = level.getBlockState(AT);
            CompoundTag tag = be.saveWithoutMetadata(server.registryAccess());
            json.beginObject();
            json.name("slots").beginObject();
            for (int i = 0; i < container.getContainerSize(); i++) {
                ItemStack item = container.getItem(i);
                if (!item.isEmpty()) {
                    json.name(Integer.toString(i)).beginArray()
                            .value(BuiltInRegistries.ITEM.getKey(item.getItem()).toString())
                            .value(item.getCount()).endArray();
                }
            }
            json.endObject();
            json.name("properties").beginObject();
            for (String name : properties.keySet()) {
                json.name(name).value(property(now, name));
            }
            json.endObject();
            json.name("fields").beginObject();
            for (JsonElement f : fields) {
                json.name(f.getAsString()).value(tag.getInt(f.getAsString()));
            }
            json.endObject();
            json.endObject();
        }
        json.endArray();
        json.endObject();
        container.clearContent();
        level.setBlock(AT, Blocks.AIR.defaultBlockState(), 3);
    }
}
