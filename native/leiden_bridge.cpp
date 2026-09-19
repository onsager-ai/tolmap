#include <igraph/igraph.h>
#include <libleidenalg/GraphHelper.h>
#include <libleidenalg/Optimiser.h>
#include <libleidenalg/RBConfigurationVertexPartition.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <exception>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
void copy_error(char *out, std::size_t capacity, const std::string &message) {
  if (out == nullptr || capacity == 0) {
    return;
  }
  const std::size_t count = std::min(capacity - 1, message.size());
  std::memcpy(out, message.data(), count);
  out[count] = '\0';
}
} // namespace

extern "C" int tolmap_leiden_partition(
    std::size_t node_count, const std::size_t *endpoints,
    const double *weights, std::size_t edge_count, double resolution,
    std::uint64_t seed, const std::size_t *initial_membership,
    std::size_t *membership_out, double *modularity_out, char *error_out,
    std::size_t error_capacity) {
  igraph_t raw_graph;
  igraph_vector_int_t raw_edges;
  bool edges_ready = false;
  bool graph_ready = false;

  try {
    if (membership_out == nullptr || modularity_out == nullptr ||
        (edge_count > 0 && (endpoints == nullptr || weights == nullptr))) {
      throw std::invalid_argument("invalid null pointer passed to Leiden bridge");
    }
    if (igraph_vector_int_init(&raw_edges, edge_count * 2) != IGRAPH_SUCCESS) {
      throw std::runtime_error("igraph_vector_int_init failed");
    }
    edges_ready = true;
    for (std::size_t i = 0; i < edge_count * 2; ++i) {
      VECTOR(raw_edges)[i] = static_cast<igraph_int_t>(endpoints[i]);
    }
    if (igraph_create(&raw_graph, &raw_edges,
                      static_cast<igraph_int_t>(node_count),
                      IGRAPH_UNDIRECTED) != IGRAPH_SUCCESS) {
      throw std::runtime_error("igraph_create failed");
    }
    graph_ready = true;

    std::vector<double> edge_weights(weights, weights + edge_count);
    std::vector<double> node_sizes(node_count, 1.0);
    Graph graph(&raw_graph, edge_weights, node_sizes);

    RBConfigurationVertexPartition *partition = nullptr;
    if (initial_membership != nullptr) {
      std::vector<std::size_t> initial(initial_membership,
                                       initial_membership + node_count);
      partition = new RBConfigurationVertexPartition(&graph, initial, resolution);
    } else {
      partition = new RBConfigurationVertexPartition(&graph, resolution);
    }

    Optimiser optimiser;
    // libleidenalg's C++ constructor and leidenalg's Python Optimiser do NOT
    // agree on their defaults, and the reference implementation is the Python
    // one: `refine_consider_comms` is ALL_NEIGH_COMMS (2) in C++ and
    // RAND_NEIGH_COMM (4) in Python. The refinement phase is where Leiden
    // differs from Louvain, so inheriting the C++ default silently runs a
    // different search -- it reproduced only 73.4% of the reference's
    // membership on scrapy before this was set. Every field `find_partition`
    // relies on is pinned here rather than left to whichever default the
    // linked version happens to carry.
    optimiser.consider_comms = Optimiser::ALL_NEIGH_COMMS;   // 2
    optimiser.refine_consider_comms = Optimiser::RAND_NEIGH_COMM;  // 4
    optimiser.optimise_routine = Optimiser::MOVE_NODES;      // 10
    optimiser.refine_routine = Optimiser::MERGE_NODES;       // 11
    optimiser.refine_partition = 1;
    optimiser.consider_empty_community = 1;
    optimiser.max_comm_size = 0;
    optimiser.set_rng_seed(static_cast<std::size_t>(seed));
    double improvement = 0.0;
    do {
      improvement = optimiser.optimise_partition(partition);
    } while (improvement > 0.0);

    const auto &membership = partition->membership();
    for (std::size_t i = 0; i < node_count; ++i) {
      membership_out[i] = membership[i];
    }

    igraph_vector_int_t ig_membership;
    if (igraph_vector_int_init(&ig_membership, node_count) != IGRAPH_SUCCESS) {
      delete partition;
      throw std::runtime_error("igraph membership allocation failed");
    }
    for (std::size_t i = 0; i < node_count; ++i) {
      VECTOR(ig_membership)[i] = static_cast<igraph_int_t>(membership[i]);
    }
    igraph_vector_t ig_weights;
    if (igraph_vector_init(&ig_weights, edge_count) != IGRAPH_SUCCESS) {
      igraph_vector_int_destroy(&ig_membership);
      delete partition;
      throw std::runtime_error("igraph weight allocation failed");
    }
    for (std::size_t i = 0; i < edge_count; ++i) {
      VECTOR(ig_weights)[i] = weights[i];
    }
    const int modularity_status =
        // VertexPartition.modularity deliberately reports topology-only
        // modularity; its optimisation quality remains weight-aware.
        igraph_modularity(&raw_graph, &ig_membership, nullptr, 1.0,
                          IGRAPH_UNDIRECTED, modularity_out);
    igraph_vector_destroy(&ig_weights);
    igraph_vector_int_destroy(&ig_membership);
    delete partition;
    if (modularity_status != IGRAPH_SUCCESS) {
      throw std::runtime_error("igraph_modularity failed");
    }

    igraph_destroy(&raw_graph);
    igraph_vector_int_destroy(&raw_edges);
    return 0;
  } catch (const std::exception &error) {
    if (graph_ready) {
      igraph_destroy(&raw_graph);
    }
    if (edges_ready) {
      igraph_vector_int_destroy(&raw_edges);
    }
    copy_error(error_out, error_capacity, error.what());
    return 1;
  } catch (...) {
    if (graph_ready) {
      igraph_destroy(&raw_graph);
    }
    if (edges_ready) {
      igraph_vector_int_destroy(&raw_edges);
    }
    copy_error(error_out, error_capacity, "unknown Leiden bridge failure");
    return 2;
  }
}
