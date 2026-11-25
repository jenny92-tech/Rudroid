// INPUT:  real pinned WSI dispatch/log headers and synthetic native function addresses
// OUTPUT:  Release dispatch/logging and explicit headless-carrier ownership assertions
// POS:    CPU-only regression; never loads a Vulkan driver or creates a Wayland object
#include "layer/private_data.hpp"
#include "util/log.hpp"
#include <cstdio>
#include <cstdlib>
#include <utility>
#ifdef RUDROID_WSI_CARRIER_TEST
#include "wsi/wayland/surface_carrier.hpp"
#include <memory>
#include <unordered_map>
#endif

#ifndef NDEBUG
#error The production logging contract must be checked with NDEBUG.
#endif

static void check(bool ok)
{
   if (!ok)
   {
      std::fputs("WSI contract fixture failed\n", stderr);
      std::exit(42);
   }
}

struct Table final : layer::dispatch_table
{
   using layer::dispatch_table::dispatch_table;
   using layer::dispatch_table::call_fn;
};

static int calls;
static VkResult result_fn(int n) { ++calls; return n == 7 ? VK_SUCCESS : VK_ERROR_UNKNOWN; }
static int value_fn(int n) { ++calls; return n + 1; }
static void void_fn(int n) { check(n == 7); ++calls; }

#ifdef RUDROID_WSI_CARRIER_TEST
static unsigned carrier_cases;
static unsigned local_live;
struct LocalSurface
{
   LocalSurface() { ++local_live; }
   ~LocalSurface() { --local_live; }
};
struct CarrierMock
{
   unsigned native_calls = 0, headless_calls = 0, destroy_calls = 0, factory_calls = 0, register_calls = 0;
   VkResult create_result = VK_SUCCESS, register_result = VK_SUCCESS;
   bool fail_local = false, null_handle = false;
   const VkWaylandSurfaceCreateInfoKHR *expected_info = nullptr;
   const VkAllocationCallbacks *expected_allocator = nullptr;
   std::unordered_map<VkSurfaceKHR, std::unique_ptr<LocalSurface>> registered;
};
static CarrierMock *carrier_mock;
// Synthetic handles belong only to these mock entrypoints, never production.
static VkInstance mock_instance() { return reinterpret_cast<VkInstance>(uintptr_t(0x1000)); }
static VkSurfaceKHR mock_surface() { return reinterpret_cast<VkSurfaceKHR>(uintptr_t(0x2000)); }
static VKAPI_ATTR VkResult VKAPI_CALL native_surface(VkInstance instance, const VkWaylandSurfaceCreateInfoKHR *info,
                                                   const VkAllocationCallbacks *allocator, VkSurfaceKHR *out)
{
   check(instance == mock_instance() && info == carrier_mock->expected_info);
   check(allocator == carrier_mock->expected_allocator);
   ++carrier_mock->native_calls;
   if (carrier_mock->create_result == VK_SUCCESS && !carrier_mock->null_handle) *out = mock_surface();
   return carrier_mock->create_result;
}
static VKAPI_ATTR VkResult VKAPI_CALL headless_surface(VkInstance instance, const VkHeadlessSurfaceCreateInfoEXT *info,
                                                     const VkAllocationCallbacks *allocator, VkSurfaceKHR *out)
{
   check(instance == mock_instance() && info->sType == VK_STRUCTURE_TYPE_HEADLESS_SURFACE_CREATE_INFO_EXT);
   check(info->pNext == nullptr && info->flags == 0 && allocator == carrier_mock->expected_allocator);
   ++carrier_mock->headless_calls;
   if (carrier_mock->create_result == VK_SUCCESS && !carrier_mock->null_handle) *out = mock_surface();
   return carrier_mock->create_result;
}
static VKAPI_ATTR void VKAPI_CALL destroy_surface(VkInstance instance, VkSurfaceKHR surface,
                                                 const VkAllocationCallbacks *allocator)
{
   check(instance == mock_instance() && surface == mock_surface());
   check(allocator == carrier_mock->expected_allocator);
   ++carrier_mock->destroy_calls;
}
static Table carrier_table(bool native, bool headless, bool destroy)
{
   util::allocator allocator{ VK_SYSTEM_ALLOCATION_SCOPE_INSTANCE, nullptr };
   using Map = layer::dispatch_table::entrypoint_list;
   using Pair = std::pair<const std::string, layer::entrypoint>;
   auto entries = allocator.make_unique<Map>(util::custom_allocator<Pair>{ allocator });
   check(bool(entries));
   auto add = [&](const char *name, PFN_vkVoidFunction fn) {
      auto result = entries->try_insert(std::make_pair(std::string(name), layer::entrypoint{ name, "", fn, 0, false, false }));
      check(result.has_value() && result->second);
   };
   add("vkCreateWaylandSurfaceKHR", native ? reinterpret_cast<PFN_vkVoidFunction>(native_surface) : nullptr);
   add("vkCreateHeadlessSurfaceEXT", headless ? reinterpret_cast<PFN_vkVoidFunction>(headless_surface) : nullptr);
   add("vkDestroySurfaceKHR", destroy ? reinterpret_cast<PFN_vkVoidFunction>(destroy_surface) : nullptr);
   return Table{ std::move(entries) };
}
static VKAPI_ATTR void *VKAPI_CALL allocate(void *, size_t size, size_t, VkSystemAllocationScope) { return std::malloc(size); }
static VKAPI_ATTR void *VKAPI_CALL reallocate(void *, void *old, size_t size, size_t, VkSystemAllocationScope) { return std::realloc(old, size); }
static VKAPI_ATTR void VKAPI_CALL deallocate(void *, void *value) { std::free(value); }
struct Scenario
{
   bool native = false, headless = true, destroy = true, opted = false, enabled = false;
   bool chain = false, flags = false, fail_local = false, null_handle = false, default_allocator = false;
   VkResult create_result = VK_SUCCESS, register_result = VK_SUCCESS, expected = VK_ERROR_EXTENSION_NOT_PRESENT;
   unsigned factories = 0, creations = 0, registrations = 0, cleanups = 0;
};
static void carrier_case(const Scenario &scenario)
{
   check(local_live == 0);
   CarrierMock mock;
   carrier_mock = &mock;
   mock.create_result = scenario.create_result;
   mock.register_result = scenario.register_result;
   mock.fail_local = scenario.fail_local;
   mock.null_handle = scenario.null_handle;
   VkAllocationCallbacks callbacks{ &mock, allocate, reallocate, deallocate, nullptr, nullptr };
   int extension_chain = 17;
   VkWaylandSurfaceCreateInfoKHR info{ VK_STRUCTURE_TYPE_WAYLAND_SURFACE_CREATE_INFO_KHR,
      scenario.chain ? &extension_chain : nullptr, VkWaylandSurfaceCreateFlagsKHR(scenario.flags ? 1 : 0), nullptr, nullptr };
   mock.expected_info = &info;
   mock.expected_allocator = scenario.default_allocator ? nullptr : &callbacks;
   auto table = carrier_table(scenario.native, scenario.headless, scenario.destroy);
   VkSurfaceKHR out = mock_surface();
   VkResult result = wsi::wayland::create_wayland_surface_carrier(table, scenario.opted, scenario.enabled,
      mock_instance(), &info, mock.expected_allocator, &out,
      [&]() {
         ++mock.factory_calls;
         return mock.fail_local ? std::unique_ptr<LocalSurface>{} : std::make_unique<LocalSurface>();
      },
      [&](VkSurfaceKHR handle, std::unique_ptr<LocalSurface> &local) {
         ++mock.register_calls;
         check(handle == mock_surface() && bool(local));
         if (mock.register_result != VK_SUCCESS) return mock.register_result;
         check(mock.registered.emplace(handle, std::move(local)).second);
         return VK_SUCCESS;
      });
   check(result == scenario.expected);
   check(mock.factory_calls == scenario.factories && mock.register_calls == scenario.registrations);
   check(mock.native_calls == (scenario.native ? scenario.creations : 0));
   check(mock.headless_calls == (scenario.native ? 0 : scenario.creations));
   check(mock.destroy_calls == scenario.cleanups);
   if (result == VK_SUCCESS)
   {
      check(out == mock_surface() && mock.registered.count(out) == 1 && local_live == 1);
      // Existing production routing uses membership in this instance map,
      // and DestroySurface pairs lower destruction with map removal.
      destroy_surface(mock_instance(), out, mock.expected_allocator);
      check(mock.registered.erase(out) == 1 && local_live == 0 && mock.destroy_calls == 1);
   }
   else check(out == VK_NULL_HANDLE && mock.registered.empty() && local_live == 0);
   ++carrier_cases;
}
static void carrier_contract()
{
   for (const char *value : { "0", "01", "true", "1" })
   {
      check(setenv("RUDROID_WSI_HEADLESS_SURFACE", value, 1) == 0);
      check(wsi::wayland::headless_surface_carrier_requested() == (std::strcmp(value, "1") == 0));
   }
   check(unsetenv("RUDROID_WSI_HEADLESS_SURFACE") == 0);
   check(!wsi::wayland::headless_surface_carrier_requested());
   struct Selection { bool requested, surface, wayland, headless, enabled; VkResult expected; };
   for (const auto &selection : {
      Selection{ false, true, true, false, false, VK_SUCCESS },
      Selection{ false, true, true, true, false, VK_SUCCESS },
      Selection{ true, true, false, false, false, VK_SUCCESS },
      Selection{ true, false, true, false, false, VK_ERROR_EXTENSION_NOT_PRESENT },
      Selection{ true, true, true, false, true, VK_SUCCESS },
      Selection{ true, true, true, true, true, VK_SUCCESS } })
   {
      util::allocator allocator{ VK_SYSTEM_ALLOCATION_SCOPE_COMMAND, nullptr };
      util::extension_list extensions{ allocator };
      if (selection.surface) check(extensions.add(VK_KHR_SURFACE_EXTENSION_NAME) == VK_SUCCESS);
      if (selection.wayland) check(extensions.add(VK_KHR_WAYLAND_SURFACE_EXTENSION_NAME) == VK_SUCCESS);
      if (selection.headless) check(extensions.add(VK_EXT_HEADLESS_SURFACE_EXTENSION_NAME) == VK_SUCCESS);
      bool enabled = true;
      check(wsi::wayland::enable_headless_surface_carrier(extensions, selection.requested, enabled) == selection.expected);
      check(enabled == selection.enabled);
      util::vector<const char *> names{ allocator };
      check(extensions.get_extension_strings(names) == VK_SUCCESS);
      unsigned count = 0;
      for (const char *name : names) if (!std::strcmp(name, VK_EXT_HEADLESS_SURFACE_EXTENSION_NAME)) ++count;
      check(count == unsigned(selection.headless || selection.enabled));
   }
   Scenario system; carrier_case(system); // A headless PFN alone never opts in.
   system.enabled = true; carrier_case(system); // Nor does an app-enabled extension.
   Scenario disabled_extension; disabled_extension.opted = true; carrier_case(disabled_extension);
   Scenario enabled; enabled.opted = enabled.enabled = true;
   enabled.expected = VK_SUCCESS; enabled.factories = enabled.creations = enabled.registrations = 1;
   carrier_case(enabled);
   auto no_headless = enabled; no_headless.headless = false; no_headless.expected = VK_ERROR_EXTENSION_NOT_PRESENT;
   no_headless.factories = no_headless.creations = no_headless.registrations = 0; carrier_case(no_headless);
   auto no_destroy = no_headless; no_destroy.headless = true; no_destroy.destroy = false; carrier_case(no_destroy);
   auto chain = enabled; chain.chain = true; chain.expected = VK_ERROR_FEATURE_NOT_PRESENT;
   chain.factories = chain.creations = chain.registrations = 0; carrier_case(chain);
   auto flags = chain; flags.chain = false; flags.flags = true; carrier_case(flags);
   auto allocation = enabled; allocation.fail_local = true; allocation.expected = VK_ERROR_OUT_OF_HOST_MEMORY;
   allocation.creations = allocation.registrations = 0; carrier_case(allocation);
   auto creation = enabled; creation.create_result = creation.expected = VK_ERROR_OUT_OF_DEVICE_MEMORY;
   creation.registrations = 0; carrier_case(creation);
   auto registration = enabled; registration.register_result = registration.expected = VK_ERROR_OUT_OF_HOST_MEMORY;
   registration.cleanups = 1; carrier_case(registration);
   auto null_handle = enabled; null_handle.null_handle = true; null_handle.expected = VK_ERROR_INITIALIZATION_FAILED;
   null_handle.registrations = 0; carrier_case(null_handle);
   auto defaults = enabled; defaults.default_allocator = true; carrier_case(defaults);
   auto native = enabled; native.native = true; carrier_case(native); // Prefer native Wayland even when opted in.
   native.opted = native.enabled = false; native.chain = true; carrier_case(native); // Preserve native create info verbatim.
   auto native_failure = native; native_failure.create_result = native_failure.expected = VK_ERROR_INITIALIZATION_FAILED;
   native_failure.registrations = 0; carrier_case(native_failure);
}
#endif

int main()
{
   util::allocator alloc{ VK_SYSTEM_ALLOCATION_SCOPE_INSTANCE, nullptr };
   using Map = layer::dispatch_table::entrypoint_list;
   using Pair = std::pair<const std::string, layer::entrypoint>;
   auto entries = alloc.make_unique<Map>(util::custom_allocator<Pair>{ alloc });
   check(bool(entries));
   auto add = [&](const char *name, PFN_vkVoidFunction fn) {
      auto inserted = entries->try_insert(std::make_pair(std::string(name),
         layer::entrypoint{ name, "", fn, 0, false, false }));
      check(inserted.has_value() && inserted->second);
   };
   add("Null", nullptr);
   add("Result", reinterpret_cast<PFN_vkVoidFunction>(result_fn));
   add("Value", reinterpret_cast<PFN_vkVoidFunction>(value_fn));
   add("Void", reinterpret_cast<PFN_vkVoidFunction>(void_fn));
   Table table{ std::move(entries) };
   using Result = VkResult (*)(int);
   using Value = int (*)(int);
   using Void = void (*)(int);
   for (const char *name : { "Absent", "Null" })
   {
      check(!table.get_fn<Result>(name).has_value());
      check(table.call_fn<Result>(name, 7) == VK_ERROR_EXTENSION_NOT_PRESENT);
      check(!table.call_fn<Value>(name, 7).has_value());
      table.call_fn<Void>(name, 7);
      check(calls == 0);
   }
   check(table.get_fn<Result>("Result").value() == result_fn);
   check(table.call_fn<Result>("Result", 7) == VK_SUCCESS);
   check(table.call_fn<Value>("Value", 7).value() == 8);
   table.call_fn<Void>("Void", 7);
   check(calls == 3);

   int evaluations = 0;
   WSI_LOG_ERROR("fixture-error-%d", ++evaluations);
   WSI_LOG_WARNING("fixture-warning-%d", ++evaluations);
   WSI_LOG_INFO("fixture-info-%d", ++evaluations);
   std::printf("evaluations=%d calls=%d\n", evaluations, calls);
#ifdef RUDROID_WSI_CARRIER_TEST
   carrier_contract();
   std::printf("carrier_cases=%u\n", carrier_cases);
#endif
}
