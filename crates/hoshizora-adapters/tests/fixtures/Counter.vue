<template>
  <button @click="increment">+</button>
  <button v-on:click.prevent="reset()">0</button>
</template>

<script setup lang="ts">
import { ref } from "vue";
const count = ref(0);
function increment() {
  count.value++;
}
const reset = () => {
  count.value = 0;
};
</script>

<style scoped>
button { color: red; }
</style>
