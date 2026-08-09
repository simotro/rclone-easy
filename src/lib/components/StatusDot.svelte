<script lang="ts">
  let { color }: { color: "green" | "yellow" | "red" | "grey" } = $props();
</script>

<span class="dot {color}"></span>

<style>
.dot {
  display: inline-block;
  width: 0.9em;
  height: 0.9em;
  flex-shrink: 0;
  border-radius: 50%;
  box-shadow: 0 0 0 1.5px var(--border-color-subtle);
}

.green {
  background-color: var(--status-green);
}

.yellow {
  background-color: var(--status-yellow);
}

.red {
  background-color: var(--status-red);
}

.grey {
  background-color: var(--status-grey);
}
</style>
