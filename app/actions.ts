'use server';

import { supabase } from "@/lib/supabase";

export async function subscribeUser(prevState: any, formData: FormData) {
  const email = formData.get("email");

  if (!email || typeof email !== "string") {
    return { success: false, error: "Please enter a valid email." };
  }

  const { error } = await supabase
    .from("subscribers")
    .insert([{ email }]);

  if (error) {
    console.error("Subscription error:", error);
    // If table doesn't exist or other db error
    if (error.code === "P0001" || error.message.includes("relation")) {
      return { 
        success: false, 
        error: "Subscribers database setup incomplete. Please create the table." 
      };
    }
    return { success: false, error: error.message };
  }

  return { success: true, message: "Thank you! We will keep you updated." };
}
